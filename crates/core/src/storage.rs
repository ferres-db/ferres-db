//! # Storage — camada de persistência em disco
//!
//! ## Decisões arquiteturais
//!
//! - **Formato JSON-lines (`.jsonl`)**: cada ponto é serializado em uma
//!   linha. Isso permite append incremental, streaming de leitura e
//!   recuperação parcial em caso de crash (linhas completas são válidas).
//!
//! - **Diretório por coleção**: cada coleção vive em
//!   `<base_path>/<collection_name>/`. Dentro há:
//!   - `points.jsonl` — dados dos pontos
//!   - `meta.json`   — configuração da coleção (dimensão, distância, HNSW params)
//!
//! - **WAL (Write-Ahead Log)**: operações são registradas em `wal.log`
//!   antes da mutação em memória. A cada 1000 operações, um snapshot
//!   completo é criado e o WAL é truncado. Ver módulo [`crate::wal`].
//!
//! ## Circuit Breaker para Disk I/O
//!
//! [`StorageCircuitBreaker`] protege contra falhas repetidas de disco (ex.: disco cheio).
//! Evita que saves falhem silenciosamente: após N falhas o circuito abre e retorna erro
//! explícito até que um timeout permita nova tentativa (HalfOpen).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::collection::{Collection, CollectionConfig};
use crate::error::FerresError;
use crate::point::Point;
use crate::search::{DistanceMetric, HnswConfig};

// ─── StorageCircuitBreaker ─────────────────────────────────────────────

/// Estados do circuit breaker para I/O em disco.
pub const CB_CLOSED: u8 = 0;
pub const CB_OPEN: u8 = 1;
pub const CB_HALF_OPEN: u8 = 2;

/// Circuit breaker para operações de storage (proteção contra disco cheio e falhas repetidas).
///
/// - **Closed**: operações passam; falhas incrementam contador.
/// - **Open**: após `failure_threshold` falhas, retorna erro imediato sem chamar disco.
/// - **HalfOpen**: após `reset_timeout_secs`, permite uma tentativa; sucesso fecha, falha reabre.
#[derive(Debug)]
pub struct StorageCircuitBreaker {
    failure_count: AtomicU64,
    last_failure: AtomicU64,
    state: AtomicU8,
    /// Número de falhas consecutivas que abrem o circuito.
    failure_threshold: u64,
    /// Segundos após abertura antes de tentar novamente (HalfOpen).
    reset_timeout_secs: u64,
}

impl StorageCircuitBreaker {
    /// Cria um circuit breaker com limites padrão (5 falhas, 60s de timeout).
    pub fn new() -> Self {
        Self::with_config(5, 60)
    }

    /// Cria um circuit breaker com threshold e timeout configuráveis.
    pub fn with_config(failure_threshold: u64, reset_timeout_secs: u64) -> Self {
        Self {
            failure_count: AtomicU64::new(0),
            last_failure: AtomicU64::new(0),
            state: AtomicU8::new(CB_CLOSED),
            failure_threshold,
            reset_timeout_secs,
        }
    }

    /// Executa a operação de I/O protegida pelo circuit breaker.
    ///
    /// Se o circuito estiver **Open**, retorna `Err(Storage("circuit breaker open"))` sem executar `f`.
    /// Se **Closed** ou **HalfOpen**, executa `f` e atualiza estado em caso de sucesso/falha.
    pub fn call<F, T>(&self, f: F) -> Result<T, FerresError>
    where
        F: FnOnce() -> Result<T, FerresError>,
    {
        let state = self.state.load(Ordering::Acquire);

        if state == CB_OPEN {
            if self.should_attempt_reset() {
                self.state.store(CB_HALF_OPEN, Ordering::Release);
                // Continua para executar f() abaixo
            } else {
                warn!("storage circuit breaker open — rejecting call");
                return Err(FerresError::Storage("circuit breaker open".into()));
            }
        }

        match f() {
            Ok(v) => {
                self.on_success();
                Ok(v)
            }
            Err(e) => {
                self.on_failure();
                Err(e)
            }
        }
    }

    fn should_attempt_reset(&self) -> bool {
        let last = self.last_failure.load(Ordering::Acquire);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        last.saturating_add(self.reset_timeout_secs) <= now
    }

    fn on_success(&self) {
        self.failure_count.store(0, Ordering::Release);
        self.state.store(CB_CLOSED, Ordering::Release);
    }

    fn on_failure(&self) {
        let prev = self.failure_count.fetch_add(1, Ordering::AcqRel);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.last_failure.store(now, Ordering::Release);

        if prev + 1 >= self.failure_threshold {
            self.state.store(CB_OPEN, Ordering::Release);
            warn!(
                failure_count = prev + 1,
                "storage circuit breaker opened after threshold"
            );
        } else if self.state.load(Ordering::Acquire) == CB_HALF_OPEN {
            self.state.store(CB_OPEN, Ordering::Release);
            warn!("storage circuit breaker re-opened after half-open failure");
        }
    }

    /// Estado atual (0=Closed, 1=Open, 2=HalfOpen). Útil para métricas e debug.
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    /// Contador de falhas consecutivas (zerado em sucesso).
    pub fn failure_count(&self) -> u64 {
        self.failure_count.load(Ordering::Acquire)
    }
}

impl Default for StorageCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadados persistidos de uma coleção.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMeta {
    pub name: String,
    pub dimension: usize,
    pub distance: DistanceMetric,
    pub hnsw_config: HnswConfig,
}

/// Engine de armazenamento em disco baseado em arquivos.
///
/// Cada instância opera sobre um diretório base; coleções são
/// subdiretórios dentro dele.
pub struct DiskStorage {
    base_path: PathBuf,
}

impl DiskStorage {
    /// Cria (ou abre) o storage no diretório indicado.
    /// O diretório é criado recursivamente se não existir.
    pub fn new(base_path: impl Into<PathBuf>) -> Result<Self, FerresError> {
        let base_path = base_path.into();
        fs::create_dir_all(&base_path).map_err(|e| FerresError::Storage(e.to_string()))?;
        info!(path = %base_path.display(), "disk storage initialized");
        Ok(Self { base_path })
    }

    /// Persiste os metadados e pontos de uma coleção.
    ///
    /// A escrita usa um arquivo temporário + rename para garantir
    /// atomicidade no nível do filesystem (evita corrupção em crash).
    pub fn save_collection(
        &self,
        meta: &CollectionMeta,
        points: &[Point],
    ) -> Result<(), FerresError> {
        let dir = self.collection_dir(&meta.name);
        fs::create_dir_all(&dir).map_err(|e| FerresError::Storage(e.to_string()))?;

        // Salva metadados
        let meta_path = dir.join("meta.json");
        let meta_json =
            serde_json::to_string_pretty(meta).map_err(|e| FerresError::Storage(e.to_string()))?;
        Self::atomic_write(&meta_path, meta_json.as_bytes())?;

        // Salva pontos em formato JSON-lines
        let points_path = dir.join("points.jsonl");
        let mut lines = String::new();
        for point in points {
            let line =
                serde_json::to_string(point).map_err(|e| FerresError::Storage(e.to_string()))?;
            lines.push_str(&line);
            lines.push('\n');
        }
        Self::atomic_write(&points_path, lines.as_bytes())?;

        debug!(
            collection = %meta.name,
            points = points.len(),
            "collection saved to disk"
        );
        Ok(())
    }

    /// Carrega uma coleção do disco, retornando metadados e pontos.
    pub fn load_collection(
        &self,
        name: &str,
    ) -> Result<(CollectionMeta, Vec<Point>), FerresError> {
        let dir = self.collection_dir(name);
        if !dir.exists() {
            return Err(FerresError::CollectionNotFound(name.to_string()));
        }

        let meta_path = dir.join("meta.json");
        let meta_bytes =
            fs::read_to_string(&meta_path).map_err(|e| FerresError::Storage(e.to_string()))?;
        let meta: CollectionMeta =
            serde_json::from_str(&meta_bytes).map_err(|e| FerresError::Storage(e.to_string()))?;

        let points_path = dir.join("points.jsonl");
        let mut points = Vec::new();

        if points_path.exists() {
            let content = fs::read_to_string(&points_path)
                .map_err(|e| FerresError::Storage(e.to_string()))?;
            for line in content.lines() {
                if line.is_empty() {
                    continue;
                }
                let point: Point = serde_json::from_str(line)
                    .map_err(|e| FerresError::Storage(e.to_string()))?;
                points.push(point);
            }
        }

        debug!(
            collection = %name,
            points = points.len(),
            "collection loaded from disk"
        );
        Ok((meta, points))
    }

    /// Lista os nomes de todas as coleções persistidas.
    pub fn list_collections(&self) -> Result<Vec<String>, FerresError> {
        let mut names = Vec::new();

        let entries =
            fs::read_dir(&self.base_path).map_err(|e| FerresError::Storage(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| FerresError::Storage(e.to_string()))?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if entry.path().join("meta.json").exists() {
                        names.push(name.to_string());
                    }
                }
            }
        }

        Ok(names)
    }

    /// Remove uma coleção do disco.
    pub fn delete_collection(&self, name: &str) -> Result<(), FerresError> {
        let dir = self.collection_dir(name);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| FerresError::Storage(e.to_string()))?;
            info!(collection = %name, "collection deleted from disk");
        }
        Ok(())
    }

    fn collection_dir(&self, name: &str) -> PathBuf {
        self.base_path.join(name)
    }

    /// Escrita atômica: grava em arquivo temporário e renomeia.
    fn atomic_write(path: &Path, data: &[u8]) -> Result<(), FerresError> {
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, data).map_err(|e| FerresError::Storage(e.to_string()))?;
        fs::rename(&tmp_path, path).map_err(|e| FerresError::Storage(e.to_string()))?;
        Ok(())
    }
}

// ─── IndexSnapshot ─────────────────────────────────────────────────

/// Snapshot binário dos metadados do índice HNSW.
///
/// Serializado com `bincode` em `index.bin`. Contém dados de validação
/// para verificar consistência entre o índice e os pontos carregados.
/// O grafo HNSW é reconstruído a partir dos pontos no load (padrão
/// em vector databases — evita acoplamento de lifetime com `hnsw_rs`).
#[derive(serde::Serialize, serde::Deserialize)]
struct IndexSnapshot {
    /// Versão do formato (para migração futura).
    version: u8,
    /// Número de pontos no momento do dump.
    point_count: usize,
    /// Dimensionalidade dos vetores.
    dimension: usize,
    /// Métrica de distância configurada.
    distance: DistanceMetric,
    /// IDs dos pontos na ordem em que estavam na coleção.
    point_ids: Vec<String>,
}

// ─── FileStorage ───────────────────────────────────────────────────

/// Persistência de coleções em disco com validação de integridade.
///
/// Opera diretamente sobre um diretório-alvo recebido como `path`.
/// Diferente de [`DiskStorage`] (que gerencia múltiplas coleções sob
/// um diretório base), `FileStorage` trabalha com uma coleção por vez
/// e aceita [`Collection`] diretamente.
///
/// ## Formato no diretório `path`
///
/// | Arquivo          | Conteúdo                                        |
/// |------------------|-------------------------------------------------|
/// | `config.json`    | [`CollectionConfig`] serializado como JSON       |
/// | `points.jsonl`   | Cada linha = 1 [`Point`] serializado como JSON   |
/// | `index.bin`      | Metadados do índice em bincode                   |
/// | `checksum.md5`   | Hash MD5 (hex) de `points.jsonl`                 |
///
/// Todas as escritas usam o padrão temp-file + rename para atomicidade.
pub struct FileStorage;

impl FileStorage {
    /// Persiste uma coleção inteira no diretório `path`.
    ///
    /// Cria o diretório se não existir. Sobrescreve arquivos existentes.
    pub fn save_collection(collection: &Collection, path: &Path) -> Result<(), FerresError> {
        fs::create_dir_all(path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to create directory {}: {e}",
                path.display()
            ))
        })?;

        // 1. config.json
        let config_json = serde_json::to_string_pretty(collection.config())
            .map_err(|e| FerresError::Storage(format!("failed to serialize config: {e}")))?;
        Self::atomic_write(&path.join("config.json"), config_json.as_bytes())?;

        // 2. points.jsonl
        let points = collection.points_owned();
        let mut lines = String::new();
        for point in &points {
            let line = serde_json::to_string(point).map_err(|e| {
                FerresError::Storage(format!("failed to serialize point {}: {e}", point.id))
            })?;
            lines.push_str(&line);
            lines.push('\n');
        }
        Self::atomic_write(&path.join("points.jsonl"), lines.as_bytes())?;

        // 3. checksum.md5
        let digest = md5::compute(lines.as_bytes());
        let checksum = format!("{:x}", digest);
        Self::atomic_write(&path.join("checksum.md5"), checksum.as_bytes())?;

        // 4. index.bin — snapshot binário de metadados do índice
        let snapshot = IndexSnapshot {
            version: 1,
            point_count: points.len(),
            dimension: collection.config().dimension,
            distance: collection.config().distance,
            point_ids: points.iter().map(|p| p.id.clone()).collect(),
        };
        let encoded = bincode::serialize(&snapshot)
            .map_err(|e| FerresError::Storage(format!("failed to serialize index: {e}")))?;
        Self::atomic_write(&path.join("index.bin"), &encoded)?;

        debug!(
            name = %collection.name(),
            points = points.len(),
            path = %path.display(),
            "collection saved via FileStorage"
        );
        Ok(())
    }

    /// Carrega uma coleção do diretório `path`.
    ///
    /// Valida o checksum MD5 de `points.jsonl` quando `checksum.md5`
    /// está presente. Reconstrói o índice HNSW a partir dos pontos.
    pub fn load_collection(path: &Path) -> Result<Collection, FerresError> {
        if !path.exists() {
            return Err(FerresError::CollectionNotFound(
                path.display().to_string(),
            ));
        }

        // 1. config.json
        let config_path = path.join("config.json");
        let config_bytes = fs::read_to_string(&config_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to read config at {}: {e}",
                config_path.display()
            ))
        })?;
        let config: CollectionConfig = serde_json::from_str(&config_bytes).map_err(|e| {
            FerresError::Storage(format!(
                "corrupted config at {}: {e}",
                config_path.display()
            ))
        })?;

        // 2. points.jsonl — lê conteúdo bruto para validação de checksum
        let points_path = path.join("points.jsonl");
        let points_content = if points_path.exists() {
            fs::read_to_string(&points_path).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to read points at {}: {e}",
                    points_path.display()
                ))
            })?
        } else {
            String::new()
        };

        // 3. checksum.md5 — validação de integridade
        let checksum_path = path.join("checksum.md5");
        if checksum_path.exists() {
            let stored = fs::read_to_string(&checksum_path)
                .map_err(|e| FerresError::Storage(format!("failed to read checksum: {e}")))?;
            let computed = format!("{:x}", md5::compute(points_content.as_bytes()));
            if stored.trim() != computed {
                return Err(FerresError::Storage(format!(
                    "integrity check failed for {}: expected MD5 {}, got {computed}",
                    points_path.display(),
                    stored.trim(),
                )));
            }
        }

        // Parse points — erro detalhado por linha para diagnóstico
        let mut points = Vec::new();
        for (line_num, line) in points_content.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let point: Point = serde_json::from_str(line).map_err(|e| {
                FerresError::Storage(format!(
                    "corrupted point at line {} in {}: {e}",
                    line_num + 1,
                    points_path.display(),
                ))
            })?;
            points.push(point);
        }

        // 4. index.bin — validação opcional de metadados
        let index_path = path.join("index.bin");
        if index_path.exists() {
            match fs::read(&index_path) {
                Ok(data) => match bincode::deserialize::<IndexSnapshot>(&data) {
                    Ok(snapshot) => {
                        if snapshot.point_count != points.len() {
                            debug!(
                                expected = snapshot.point_count,
                                got = points.len(),
                                "index.bin point count mismatch — rebuilding"
                            );
                        }
                        if snapshot.dimension != config.dimension {
                            debug!(
                                expected = snapshot.dimension,
                                got = config.dimension,
                                "index.bin dimension mismatch — rebuilding"
                            );
                        }
                    }
                    Err(e) => {
                        debug!(
                            "index.bin corrupted ({e}), ignoring — will rebuild from points"
                        );
                    }
                },
                Err(e) => {
                    debug!("failed to read index.bin ({e}), ignoring — will rebuild from points");
                }
            }
        }

        // Reconstrói a Collection (inclui rebuild do índice HNSW)
        let collection = Collection::from_points(config, points)?;

        debug!(
            name = %collection.name(),
            points = collection.len(),
            path = %path.display(),
            "collection loaded via FileStorage"
        );
        Ok(collection)
    }

    /// Escrita atômica: grava em `.tmp` e depois faz rename.
    fn atomic_write(path: &Path, data: &[u8]) -> Result<(), FerresError> {
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, data).map_err(|e| {
            FerresError::Storage(format!(
                "failed to write temp file {}: {e}",
                tmp_path.display()
            ))
        })?;
        fs::rename(&tmp_path, path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to rename {} → {}: {e}",
                tmp_path.display(),
                path.display()
            ))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = std::env::temp_dir().join("ferres_test_storage_v2");
        let _ = fs::remove_dir_all(&tmp);
        let storage = DiskStorage::new(&tmp).unwrap();

        let meta = CollectionMeta {
            name: "test_col".to_string(),
            dimension: 3,
            distance: DistanceMetric::Cosine,
            hnsw_config: HnswConfig::default(),
        };

        let points = vec![
            Point::new("p1", vec![1.0, 2.0, 3.0], serde_json::json!({"a": 1})).unwrap(),
            Point::new("p2", vec![4.0, 5.0, 6.0], serde_json::Value::Null).unwrap(),
        ];

        storage.save_collection(&meta, &points).unwrap();

        let (loaded_meta, loaded_points) = storage.load_collection("test_col").unwrap();
        assert_eq!(loaded_meta.name, "test_col");
        assert_eq!(loaded_meta.dimension, 3);
        assert_eq!(loaded_meta.distance, DistanceMetric::Cosine);
        assert_eq!(loaded_points.len(), 2);
        assert_eq!(loaded_points[0].id, "p1");

        let _ = fs::remove_dir_all(&tmp);
    }

    // ─── FileStorage tests ──────────────────────────────────────────

    #[test]
    fn file_storage_roundtrip_100_points() {
        let tmp = std::env::temp_dir().join("ferres_test_file_storage_rt");
        let _ = fs::remove_dir_all(&tmp);

        let config = CollectionConfig {
            name: "roundtrip_test".to_string(),
            dimension: 8,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
        };

        let mut collection = Collection::new(config);
        for i in 0..100 {
            let vector: Vec<f32> = (0..8).map(|d| (i * 8 + d) as f32 / 800.0).collect();
            let meta = serde_json::json!({"idx": i});
            let point = Point::new(format!("pt-{i}"), vector, meta).unwrap();
            collection.insert(point).unwrap();
        }

        // Save
        FileStorage::save_collection(&collection, &tmp).unwrap();

        // Verify all 4 files exist
        assert!(tmp.join("config.json").exists());
        assert!(tmp.join("points.jsonl").exists());
        assert!(tmp.join("index.bin").exists());
        assert!(tmp.join("checksum.md5").exists());

        // Load
        let loaded = FileStorage::load_collection(&tmp).unwrap();

        // Validate config
        assert_eq!(loaded.config().name, "roundtrip_test");
        assert_eq!(loaded.config().dimension, 8);
        assert_eq!(loaded.config().distance, DistanceMetric::Euclidean);

        // Validate all 100 points
        assert_eq!(loaded.len(), 100);
        for i in 0..100 {
            let id = format!("pt-{i}");
            let original = collection.get(&id).expect("original point missing");
            let restored = loaded.get(&id).expect("restored point missing");
            assert_eq!(original.id, restored.id);
            assert_eq!(original.vector, restored.vector);
            assert_eq!(original.metadata, restored.metadata);
            assert_eq!(original.created_at, restored.created_at);
        }

        // Validate search still works after reload
        let query = collection.get("pt-0").unwrap().vector.clone();
        let results = loaded.search(&query, 5).unwrap();
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0, "pt-0");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_storage_detects_corrupted_points() {
        let tmp = std::env::temp_dir().join("ferres_test_corrupted_pts");
        let _ = fs::remove_dir_all(&tmp);

        let config = CollectionConfig {
            name: "corrupt_test".to_string(),
            dimension: 2,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
        };
        let mut collection = Collection::new(config);
        collection
            .insert(Point::new("p1", vec![1.0, 2.0], serde_json::Value::Null).unwrap())
            .unwrap();

        FileStorage::save_collection(&collection, &tmp).unwrap();

        // Corrupt the points file — checksum will no longer match
        fs::write(tmp.join("points.jsonl"), b"this is not valid json\n").unwrap();

        let result = FileStorage::load_collection(&tmp);
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("integrity check failed"),
            "expected integrity error, got: {err}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_storage_handles_corrupted_config() {
        let tmp = std::env::temp_dir().join("ferres_test_corrupted_cfg");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // Write a corrupted config.json
        fs::write(tmp.join("config.json"), b"{ invalid json }").unwrap();
        fs::write(tmp.join("points.jsonl"), b"").unwrap();

        let result = FileStorage::load_collection(&tmp);
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("corrupted config"),
            "expected corrupted config error, got: {err}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_storage_missing_directory_returns_not_found() {
        let result = FileStorage::load_collection(Path::new("/nonexistent/path/nowhere"));
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("not found") || err.contains("collection not found"),
            "expected not found error, got: {err}"
        );
    }

    // ─── DiskStorage tests ─────────────────────────────────────────

    #[test]
    fn list_and_delete_collections() {
        let tmp = std::env::temp_dir().join("ferres_test_list_v2");
        let _ = fs::remove_dir_all(&tmp);
        let storage = DiskStorage::new(&tmp).unwrap();

        let meta = CollectionMeta {
            name: "col_a".to_string(),
            dimension: 2,
            distance: DistanceMetric::Euclidean,
            hnsw_config: HnswConfig::default(),
        };
        storage.save_collection(&meta, &[]).unwrap();

        let names = storage.list_collections().unwrap();
        assert!(names.contains(&"col_a".to_string()));

        storage.delete_collection("col_a").unwrap();
        let names = storage.list_collections().unwrap();
        assert!(!names.contains(&"col_a".to_string()));

        let _ = fs::remove_dir_all(&tmp);
    }

    // ─── StorageCircuitBreaker tests ───────────────────────────────────

    #[test]
    fn circuit_breaker_passes_through_ok() {
        let cb = StorageCircuitBreaker::with_config(3, 60);
        let r = cb.call(|| Ok::<_, FerresError>(42));
        assert!(matches!(r, Ok(42)));
        assert_eq!(cb.state(), CB_CLOSED);
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let cb = StorageCircuitBreaker::with_config(3, 60);

        for _ in 0..3 {
            let r = cb.call(|| Err::<(), _>(FerresError::Storage("disk full".into())));
            assert!(r.is_err());
        }
        assert_eq!(cb.state(), CB_OPEN);
        assert_eq!(cb.failure_count(), 3);

        // Próxima chamada não executa o closure e retorna "circuit breaker open"
        let mut called = false;
        let r = cb.call(|| {
            called = true;
            Ok(())
        });
        assert!(!called, "closure must not run when circuit is open");
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("circuit breaker open"), "got: {msg}");
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let cb = StorageCircuitBreaker::with_config(2, 60);
        cb.call(|| Err::<(), _>(FerresError::Storage("fail".into()))).ok();
        assert_eq!(cb.failure_count(), 1);
        cb.call(|| Ok(())).unwrap();
        assert_eq!(cb.state(), CB_CLOSED);
        assert_eq!(cb.failure_count(), 0);
    }
}
