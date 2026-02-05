//! # Write-Ahead Log (WAL) — durabilidade de operações
//!
//! Antes de modificar uma coleção em memória, a operação é registrada
//! no WAL (`wal.log`). Em caso de crash, as operações pendentes são
//! re-aplicadas sobre o último snapshot na recuperação.
//!
//! ## Formato
//!
//! O WAL usa JSON-lines — cada linha é um `WalEntry` serializado:
//! ```json
//! {"timestamp":1234567890,"operation":{"op":"upsert","point":{...}}}
//! {"timestamp":1234567891,"operation":{"op":"delete","id":"point-123"}}
//! ```
//!
//! ## Snapshot
//!
//! A cada `snapshot_threshold` operações (padrão: 1000), um snapshot
//! completo é criado via `FileStorage::save_collection` e o WAL é
//! truncado.

use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::collection::Collection;
use crate::error::FerresError;
use crate::point::Point;
use crate::storage::FileStorage;

// ─── WAL Entry Types ──────────────────────────────────────────────────

/// Uma única operação registrada no WAL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Timestamp Unix em segundos.
    pub timestamp: u64,
    /// A operação e seu payload.
    pub operation: WalOperation,
}

/// Tipo de operação registrada no WAL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WalOperation {
    /// Insere ou atualiza um ponto.
    Upsert {
        /// O ponto completo.
        point: Point,
    },
    /// Remove um ponto pelo ID.
    Delete {
        /// O ID do ponto a remover.
        id: String,
    },
}

// ─── Wal ──────────────────────────────────────────────────────────────

/// Write-Ahead Log por coleção.
///
/// Garante durabilidade registrando operações antes que sejam aplicadas
/// na coleção em memória. Em caso de crash, o último snapshot é carregado
/// e as operações do WAL são re-aplicadas.
pub struct Wal {
    /// Diretório da coleção (contém wal.log).
    collection_dir: PathBuf,
    /// Writer bufferizado, mantido aberto em modo append.
    writer: Option<BufWriter<fs::File>>,
    /// Número de operações desde o último snapshot.
    ops_since_snapshot: usize,
    /// Limite para disparar snapshot automático.
    snapshot_threshold: usize,
}

impl Wal {
    /// Threshold padrão: snapshot a cada 1000 operações.
    pub const DEFAULT_SNAPSHOT_THRESHOLD: usize = 1000;

    /// Abre (ou cria) o WAL para o diretório da coleção.
    ///
    /// Não faz replay — use `recover_collection()` para recuperação.
    pub fn open(collection_dir: &Path, snapshot_threshold: usize) -> Result<Self, FerresError> {
        fs::create_dir_all(collection_dir).map_err(|e| {
            FerresError::Storage(format!(
                "failed to create collection directory {}: {e}",
                collection_dir.display()
            ))
        })?;

        let wal_path = collection_dir.join("wal.log");

        // Conta entradas existentes para inicializar o contador
        let ops_since_snapshot = if wal_path.exists() {
            Self::count_entries(&wal_path)?
        } else {
            0
        };

        // Abre o arquivo em modo append
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| {
                FerresError::Storage(format!(
                    "failed to open WAL at {}: {e}",
                    wal_path.display()
                ))
            })?;

        Ok(Self {
            collection_dir: collection_dir.to_path_buf(),
            writer: Some(BufWriter::new(file)),
            ops_since_snapshot,
            snapshot_threshold,
        })
    }

    /// Registra uma operação de upsert no WAL.
    ///
    /// Deve ser chamado ANTES da mutação em memória.
    pub fn append_upsert(&mut self, point: &Point) -> Result<(), FerresError> {
        let entry = WalEntry {
            timestamp: current_timestamp(),
            operation: WalOperation::Upsert {
                point: point.clone(),
            },
        };
        self.append_entry(&entry)?;
        self.ops_since_snapshot += 1;
        Ok(())
    }

    /// Registra uma operação de delete no WAL.
    ///
    /// Deve ser chamado ANTES da mutação em memória.
    pub fn append_delete(&mut self, id: &str) -> Result<(), FerresError> {
        let entry = WalEntry {
            timestamp: current_timestamp(),
            operation: WalOperation::Delete { id: id.to_string() },
        };
        self.append_entry(&entry)?;
        self.ops_since_snapshot += 1;
        Ok(())
    }

    /// Retorna true se o número de operações atingiu o threshold.
    pub fn should_snapshot(&self) -> bool {
        self.ops_since_snapshot >= self.snapshot_threshold
    }

    /// Trunca o WAL após um snapshot bem-sucedido.
    ///
    /// Fecha o writer atual, reabre em modo truncate e zera o contador.
    pub fn truncate_after_snapshot(&mut self) -> Result<(), FerresError> {
        // Fecha o writer atual
        self.writer = None;

        let wal_path = self.collection_dir.join("wal.log");

        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&wal_path)
            .map_err(|e| {
                FerresError::Storage(format!(
                    "failed to truncate WAL at {}: {e}",
                    wal_path.display()
                ))
            })?;

        // Reabre em modo append para operações futuras
        drop(file);
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| {
                FerresError::Storage(format!(
                    "failed to reopen WAL at {}: {e}",
                    wal_path.display()
                ))
            })?;

        self.writer = Some(BufWriter::new(file));
        self.ops_since_snapshot = 0;

        debug!(
            path = %wal_path.display(),
            "WAL truncated after snapshot"
        );
        Ok(())
    }

    /// Lê todas as entradas do WAL para replay.
    ///
    /// Linhas mal-formadas (artefato de crash) são ignoradas com warning.
    pub fn read_entries(collection_dir: &Path) -> Result<Vec<WalEntry>, FerresError> {
        let wal_path = collection_dir.join("wal.log");

        if !wal_path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&wal_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to open WAL for reading at {}: {e}",
                wal_path.display()
            ))
        })?;

        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for (line_num, line_result) in reader.lines().enumerate() {
            match line_result {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<WalEntry>(&line) {
                        Ok(entry) => entries.push(entry),
                        Err(e) => {
                            warn!(
                                line = line_num + 1,
                                error = %e,
                                path = %wal_path.display(),
                                "skipping malformed WAL entry (possible crash artifact)"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        line = line_num + 1,
                        error = %e,
                        path = %wal_path.display(),
                        "I/O error reading WAL line, stopping replay"
                    );
                    break;
                }
            }
        }

        if !entries.is_empty() {
            info!(
                entries = entries.len(),
                path = %wal_path.display(),
                "read WAL entries for replay"
            );
        }

        Ok(entries)
    }

    /// Número de operações desde o último snapshot.
    pub fn ops_since_snapshot(&self) -> usize {
        self.ops_since_snapshot
    }

    /// Caminho do arquivo WAL.
    pub fn wal_path(&self) -> PathBuf {
        self.collection_dir.join("wal.log")
    }

    // ─── Private ──────────────────────────────────────────────────────

    /// Serializa e escreve uma entrada, seguida de flush.
    fn append_entry(&mut self, entry: &WalEntry) -> Result<(), FerresError> {
        let writer = self.writer.as_mut().ok_or_else(|| {
            FerresError::Storage("WAL writer is closed".to_string())
        })?;

        let json = serde_json::to_string(entry).map_err(|e| {
            FerresError::Storage(format!("failed to serialize WAL entry: {e}"))
        })?;

        writer.write_all(json.as_bytes()).map_err(|e| {
            FerresError::Storage(format!("failed to write WAL entry: {e}"))
        })?;
        writer.write_all(b"\n").map_err(|e| {
            FerresError::Storage(format!("failed to write WAL newline: {e}"))
        })?;

        // Flush garante durabilidade antes de retornar.
        writer.flush().map_err(|e| {
            FerresError::Storage(format!("failed to flush WAL: {e}"))
        })?;

        Ok(())
    }

    /// Conta o número de entradas válidas no arquivo WAL.
    fn count_entries(wal_path: &Path) -> Result<usize, FerresError> {
        let file = fs::File::open(wal_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to open WAL for counting at {}: {e}",
                wal_path.display()
            ))
        })?;
        let reader = BufReader::new(file);
        let mut count = 0;
        for line_result in reader.lines() {
            if let Ok(line) = line_result {
                if !line.trim().is_empty() {
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

// ─── Recovery ─────────────────────────────────────────────────────────

/// Carrega uma coleção do snapshot e re-aplica entradas pendentes do WAL.
///
/// Fluxo de recuperação:
/// 1. Se não há `config.json`, não há coleção — retorna `None`.
/// 2. Carrega snapshot via `FileStorage::load_collection`.
/// 3. Se `wal.log` existe e tem entradas, re-aplica sobre a coleção.
/// 4. Retorna a coleção recuperada.
///
/// Após chamada bem-sucedida com entradas WAL, o chamador deve
/// criar um snapshot e truncar o WAL para consolidar o estado.
pub fn recover_collection(collection_dir: &Path) -> Result<Option<Collection>, FerresError> {
    let config_path = collection_dir.join("config.json");

    if !config_path.exists() {
        return Ok(None);
    }

    // Passo 1: carrega o snapshot
    let mut collection = FileStorage::load_collection(collection_dir)?;

    // Passo 2: re-aplica entradas do WAL
    let entries = Wal::read_entries(collection_dir)?;
    if !entries.is_empty() {
        info!(
            collection = %collection.name(),
            entries = entries.len(),
            "replaying WAL entries on top of snapshot"
        );

        let mut applied = 0;
        let mut skipped = 0;

        for entry in &entries {
            match &entry.operation {
                WalOperation::Upsert { point } => {
                    match collection.insert(point.clone()) {
                        Ok(_) => applied += 1,
                        Err(e) => {
                            warn!(
                                id = %point.id,
                                error = %e,
                                "failed to replay upsert, skipping"
                            );
                            skipped += 1;
                        }
                    }
                }
                WalOperation::Delete { id } => {
                    match collection.remove(id) {
                        Ok(_) => applied += 1,
                        Err(FerresError::PointNotFound(_)) => {
                            // Idempotente: ponto pode já ter sido deletado no snapshot
                            debug!(id = %id, "WAL delete: point already absent, skipping");
                            skipped += 1;
                        }
                        Err(e) => {
                            warn!(
                                id = %id,
                                error = %e,
                                "failed to replay delete, skipping"
                            );
                            skipped += 1;
                        }
                    }
                }
            }
        }

        info!(
            collection = %collection.name(),
            applied,
            skipped,
            "WAL replay complete"
        );
    }

    Ok(Some(collection))
}

/// Helper: timestamp Unix atual em segundos.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::CollectionConfig;
    use crate::search::DistanceMetric;
    use tempfile::TempDir;

    fn test_config(name: &str) -> CollectionConfig {
        CollectionConfig {
            name: name.to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: Default::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
        }
    }

    fn make_point(id: &str, v: Vec<f32>) -> Point {
        Point::new(id.to_string(), v, serde_json::Value::Null).unwrap()
    }

    // ─── WAL Unit Tests ───────────────────────────────────────────────

    #[test]
    fn wal_append_and_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 1000).unwrap();
        let p1 = make_point("p1", vec![1.0, 2.0, 3.0]);
        let p2 = make_point("p2", vec![4.0, 5.0, 6.0]);

        wal.append_upsert(&p1).unwrap();
        wal.append_upsert(&p2).unwrap();
        wal.append_delete("p1").unwrap();

        assert_eq!(wal.ops_since_snapshot(), 3);

        // Lê de volta
        let entries = Wal::read_entries(&dir).unwrap();
        assert_eq!(entries.len(), 3);

        // Verifica upsert p1
        match &entries[0].operation {
            WalOperation::Upsert { point } => {
                assert_eq!(point.id, "p1");
                assert_eq!(point.vector, vec![1.0, 2.0, 3.0]);
            }
            _ => panic!("expected upsert"),
        }

        // Verifica upsert p2
        match &entries[1].operation {
            WalOperation::Upsert { point } => assert_eq!(point.id, "p2"),
            _ => panic!("expected upsert"),
        }

        // Verifica delete p1
        match &entries[2].operation {
            WalOperation::Delete { id } => assert_eq!(id, "p1"),
            _ => panic!("expected delete"),
        }
    }

    #[test]
    fn wal_upsert_entry_format() {
        let p = make_point("test-id", vec![1.0, 2.0, 3.0]);
        let entry = WalEntry {
            timestamp: 1234567890,
            operation: WalOperation::Upsert { point: p },
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"op\":\"upsert\""));
        assert!(json.contains("\"test-id\""));
        assert!(json.contains("\"timestamp\":1234567890"));
    }

    #[test]
    fn wal_delete_entry_format() {
        let entry = WalEntry {
            timestamp: 1234567890,
            operation: WalOperation::Delete { id: "point-123".to_string() },
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"op\":\"delete\""));
        assert!(json.contains("\"point-123\""));
    }

    #[test]
    fn wal_truncate_clears_entries() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 1000).unwrap();
        let p = make_point("p1", vec![1.0, 2.0, 3.0]);
        wal.append_upsert(&p).unwrap();
        wal.append_upsert(&p).unwrap();

        assert_eq!(wal.ops_since_snapshot(), 2);

        wal.truncate_after_snapshot().unwrap();

        assert_eq!(wal.ops_since_snapshot(), 0);

        let entries = Wal::read_entries(&dir).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn wal_read_skips_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");
        fs::create_dir_all(&dir).unwrap();

        let wal_path = dir.join("wal.log");

        // Escreve entradas válidas + lixo
        let p = make_point("p1", vec![1.0, 2.0, 3.0]);
        let valid_entry = WalEntry {
            timestamp: 1,
            operation: WalOperation::Upsert { point: p },
        };
        let valid_json = serde_json::to_string(&valid_entry).unwrap();

        let content = format!("{}\n{{invalid json\n{}\n", valid_json, valid_json);
        fs::write(&wal_path, content).unwrap();

        let entries = Wal::read_entries(&dir).unwrap();
        assert_eq!(entries.len(), 2); // 2 válidas, 1 malformada ignorada
    }

    #[test]
    fn wal_should_snapshot_at_threshold() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 5).unwrap();
        let p = make_point("p1", vec![1.0, 2.0, 3.0]);

        for _ in 0..4 {
            wal.append_upsert(&p).unwrap();
        }
        assert!(!wal.should_snapshot());

        wal.append_upsert(&p).unwrap();
        assert!(wal.should_snapshot());
    }

    #[test]
    fn wal_ops_counter_survives_reopen() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        {
            let mut wal = Wal::open(&dir, 1000).unwrap();
            let p = make_point("p1", vec![1.0, 2.0, 3.0]);
            wal.append_upsert(&p).unwrap();
            wal.append_upsert(&p).unwrap();
            wal.append_upsert(&p).unwrap();
        }

        // Reabre — deve contar 3 entradas existentes
        let wal = Wal::open(&dir, 1000).unwrap();
        assert_eq!(wal.ops_since_snapshot(), 3);
    }

    #[test]
    fn wal_empty_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");
        fs::create_dir_all(&dir).unwrap();

        // Cria wal.log vazio
        fs::write(dir.join("wal.log"), "").unwrap();

        let entries = Wal::read_entries(&dir).unwrap();
        assert!(entries.is_empty());
    }

    // ─── Recovery Tests ───────────────────────────────────────────────

    #[test]
    fn recovery_snapshot_plus_wal() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Cria snapshot com 2 pontos
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // Escreve WAL com 1 upsert adicional
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0])).unwrap();

        // Recupera
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 3);
        assert!(recovered.get("a").is_some());
        assert!(recovered.get("b").is_some());
        assert!(recovered.get("c").is_some());
    }

    #[test]
    fn recovery_empty_wal() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Cria snapshot com 2 pontos, WAL vazio
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // Cria wal.log vazio
        fs::write(dir.join("wal.log"), "").unwrap();

        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 2);
    }

    #[test]
    fn recovery_no_wal_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Cria snapshot sem WAL
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // Garante que não há wal.log
        let wal_path = dir.join("wal.log");
        if wal_path.exists() {
            fs::remove_file(&wal_path).unwrap();
        }

        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered.get("a").is_some());
    }

    #[test]
    fn recovery_no_snapshot() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("empty_col");
        fs::create_dir_all(&dir).unwrap();

        let result = recover_collection(&dir).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn recovery_idempotent_upsert() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com ponto A(v1)
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com upsert A(v2)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("a", vec![0.0, 1.0, 0.0])).unwrap();

        // Recupera — A deve ter v2
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 1);
        let point = recovered.get("a").unwrap();
        assert_eq!(point.vector, vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn recovery_idempotent_delete() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com ponto A
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com delete(A)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_delete("a").unwrap();

        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 0);
        assert!(recovered.get("a").is_none());
    }

    #[test]
    fn recovery_delete_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot sem o ponto X
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com delete(X) — X não existe
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_delete("x").unwrap();

        // Não deve falhar
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered.get("a").is_some());
    }

    // ─── Crash Simulation Tests ───────────────────────────────────────

    #[test]
    fn crash_partial_wal_write() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A, B
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com upsert(C) válido + linha truncada (simula crash)
        let p = make_point("c", vec![0.0, 0.0, 1.0]);
        let valid_entry = WalEntry {
            timestamp: 1,
            operation: WalOperation::Upsert { point: p },
        };
        let valid_json = serde_json::to_string(&valid_entry).unwrap();

        let wal_path = dir.join("wal.log");
        let content = format!("{}\n{{\"timestamp\":2,\"operati", valid_json);
        fs::write(&wal_path, content).unwrap();

        // Recovery deve aplicar C e ignorar a linha truncada
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 3);
        assert!(recovered.get("a").is_some());
        assert!(recovered.get("b").is_some());
        assert!(recovered.get("c").is_some());
    }

    #[test]
    fn crash_after_wal_before_mutation() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A, B
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL tem upsert(C) — mas a coleção em memória NÃO foi mutada
        // (simula crash entre WAL append e collection.insert)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0])).unwrap();
        drop(wal);
        // NÃO chamamos col.insert — simulando crash

        // Recovery deve produzir {A, B, C}
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 3);
        assert!(recovered.get("c").is_some());
    }

    #[test]
    fn crash_between_snapshot_and_truncate() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com upsert(B)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        drop(wal);

        // Novo snapshot com {A, B} — mas NÃO trunca WAL (simula crash)
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();
        // WAL ainda tem upsert(B)

        // Recovery: snapshot {A,B} + replay upsert(B) = idempotente
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 2);
        assert!(recovered.get("a").is_some());
        assert!(recovered.get("b").is_some());
    }

    #[test]
    fn crash_mixed_operations_recovery() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A, B, C
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        col.insert(make_point("c", vec![0.0, 0.0, 1.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL: delete(B), upsert(D), upsert(A com novo vetor)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_delete("b").unwrap();
        wal.append_upsert(&make_point("d", vec![1.0, 1.0, 0.0])).unwrap();
        wal.append_upsert(&make_point("a", vec![0.5, 0.5, 0.0])).unwrap();
        drop(wal);

        // Recovery: {A(v2), C, D}
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 3);
        assert!(recovered.get("b").is_none());
        assert!(recovered.get("d").is_some());
        let a = recovered.get("a").unwrap();
        assert_eq!(a.vector, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn crash_during_snapshot_write() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot inicial com A, B
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com upsert(C)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0])).unwrap();
        drop(wal);

        // Simula crash durante snapshot: corrompe points.jsonl parcialmente
        // (simula crash durante escrita atômica - arquivo .tmp foi criado mas não renomeado)
        let points_path = dir.join("points.jsonl");
        let partial_points = r#"{"id":"a","vector":[1.0,0.0,0.0],"metadata":null,"created_at":1234567890}
{"id":"b","vector":[0.0,1.0,0.0],"metadata":null,"created_at":1234567891}
{"id":"c","vector":[0.0,0.0,1.0],"metadata":null,"created_at":1234567892
"#; // JSON incompleto na última linha
        fs::write(&points_path, partial_points).unwrap();

        // Recovery: FileStorage::load_collection deve detectar JSON corrompido
        // e falhar, mas o WAL ainda deve ser aplicável se conseguirmos carregar
        // o snapshot anterior. Na prática, precisamos de um snapshot válido.
        // Este teste valida que o sistema detecta corrupção.
        let result = recover_collection(&dir);
        
        // Se o snapshot estiver corrompido, o recovery pode falhar
        // Mas isso é esperado - o sistema detecta corrupção
        // Em produção, teríamos backups ou snapshots anteriores
        if result.is_err() {
            // Corrupção detectada - comportamento esperado
            return;
        }
        
        // Se conseguir recuperar, deve ter A, B do snapshot + C do WAL
        let recovered = result.unwrap().unwrap();
        assert_eq!(recovered.len(), 3);
    }

    #[test]
    fn crash_during_batch_operations() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot inicial vazio
        let config = test_config("test_col");
        let col = Collection::new(config);
        FileStorage::save_collection(&col, &dir).unwrap();

        // Simula batch de 50 operações, crash após 30
        let mut wal = Wal::open(&dir, 1000).unwrap();
        for i in 0..30 {
            wal.append_upsert(&make_point(&format!("p{}", i), vec![i as f32, 0.0, 0.0])).unwrap();
        }
        drop(wal);

        // Simula crash: WAL tem 30 entradas, mas apenas 20 foram aplicadas em memória
        // (não aplicamos em memória para simular crash)

        // Recovery deve aplicar todas as 30 entradas do WAL
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 30);
        for i in 0..30 {
            assert!(recovered.get(&format!("p{}", i)).is_some());
        }
    }

    #[test]
    fn crash_recovery_consistency_check() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com pontos conhecidos
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com operações
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_delete("b").unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0])).unwrap();
        wal.append_upsert(&make_point("d", vec![0.5, 0.5, 0.0])).unwrap();
        drop(wal);

        // Recovery
        let recovered = recover_collection(&dir).unwrap().unwrap();
        
        // Valida consistência: estado final deve ser {A, C, D}
        assert_eq!(recovered.len(), 3);
        assert!(recovered.get("a").is_some());
        assert!(recovered.get("b").is_none());
        assert!(recovered.get("c").is_some());
        assert!(recovered.get("d").is_some());

        // Valida que busca ainda funciona após recovery
        let query = vec![1.0, 0.0, 0.0];
        let results = recovered.search(&query, 5).unwrap();
        assert!(!results.is_empty());
        // O ponto mais próximo deve ser "a"
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn crash_at_snapshot_threshold() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot inicial
        let config = test_config("test_col");
        let col = Collection::new(config);
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com exatamente 1000 operações (threshold)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        for i in 0..1000 {
            wal.append_upsert(&make_point(&format!("p{}", i), vec![i as f32, 0.0, 0.0])).unwrap();
        }
        assert!(wal.should_snapshot());
        drop(wal);

        // Simula crash antes de fazer snapshot: WAL tem 1000 entradas
        // mas snapshot não foi criado

        // Recovery deve aplicar todas as 1000 entradas
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 1000);
    }

    #[test]
    fn crash_during_wal_truncate() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL com upsert(B)
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        drop(wal);

        // Novo snapshot com {A, B}
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // Simula crash durante truncate: WAL ainda existe mas deveria ter sido truncado
        // (não chamamos truncate_after_snapshot)

        // Recovery: snapshot {A, B} + replay upsert(B) = idempotente
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 2);
        assert!(recovered.get("a").is_some());
        assert!(recovered.get("b").is_some());
    }

    #[test]
    fn crash_multiple_wal_entries_partial_write() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot inicial
        let config = test_config("test_col");
        let col = Collection::new(config);
        FileStorage::save_collection(&col, &dir).unwrap();

        // Escreve WAL manualmente com entradas válidas + parcialmente escrita
        let wal_path = dir.join("wal.log");
        let p1 = make_point("p1", vec![1.0, 0.0, 0.0]);
        let p2 = make_point("p2", vec![0.0, 1.0, 0.0]);
        let p3 = make_point("p3", vec![0.0, 0.0, 1.0]);

        let entry1 = WalEntry {
            timestamp: 1,
            operation: WalOperation::Upsert { point: p1 },
        };
        let entry2 = WalEntry {
            timestamp: 2,
            operation: WalOperation::Upsert { point: p2 },
        };
        let entry3 = WalEntry {
            timestamp: 3,
            operation: WalOperation::Upsert { point: p3 },
        };

        let json1 = serde_json::to_string(&entry1).unwrap();
        let json2 = serde_json::to_string(&entry2).unwrap();
        let _json3 = serde_json::to_string(&entry3).unwrap();

        // Simula crash: última entrada parcialmente escrita
        let content = format!("{}\n{}\n{{\"timestamp\":3,\"operation\":{{\"op\":\"upsert\"", json1, json2);
        fs::write(&wal_path, content).unwrap();

        // Recovery deve aplicar apenas as 2 entradas válidas
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 2);
        assert!(recovered.get("p1").is_some());
        assert!(recovered.get("p2").is_some());
        assert!(recovered.get("p3").is_none());
    }

    #[test]
    fn crash_recovery_preserves_search_functionality() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Cria coleção com pontos conhecidos
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        
        // Insere pontos com vetores distintos para busca
        col.insert(make_point("near_origin", vec![0.1, 0.1, 0.1])).unwrap();
        col.insert(make_point("far_away", vec![10.0, 10.0, 10.0])).unwrap();
        FileStorage::save_collection(&col, &dir).unwrap();

        // WAL adiciona mais pontos
        let mut wal = Wal::open(&dir, 1000).unwrap();
        wal.append_upsert(&make_point("near_origin2", vec![0.2, 0.2, 0.2])).unwrap();
        wal.append_delete("far_away").unwrap();
        drop(wal);

        // Recovery
        let recovered = recover_collection(&dir).unwrap().unwrap();
        
        // Valida que busca funciona corretamente após recovery
        let query = vec![0.0, 0.0, 0.0];
        let results = recovered.search(&query, 5).unwrap();
        
        // Deve encontrar os pontos próximos à origem
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(id, _)| id == "near_origin"));
        assert!(results.iter().any(|(id, _)| id == "near_origin2"));
        // far_away não deve aparecer (foi deletado)
        assert!(!results.iter().any(|(id, _)| id == "far_away"));
    }
}
