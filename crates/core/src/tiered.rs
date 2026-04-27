//! # Tiered Storage — movimentação automática de vetores entre camadas de armazenamento
//!
//! Implementa armazenamento em três camadas (Hot, Warm, Cold) com promoção/demoção
//! automática baseada na frequência de acesso:
//!
//! - **Hot** (RAM): pontos completos em memória, acesso instantâneo.
//! - **Warm** (mmap): vetores em arquivo memory-mapped, metadata em memória.
//! - **Cold** (disco): apenas IDs em memória, dados carregados sob demanda.
//!
//! ## Decisões arquiteturais
//!
//! - O grafo HNSW **sempre** permanece em memória (apenas dados dos pontos são tiered).
//! - Tiered storage é **opt-in** via `TieredStorageConfig` (default: desabilitado).
//! - Compactação roda em background sem bloquear buscas.
//! - Qualquer acesso a um ponto Cold/Warm o promove automaticamente para Hot.

use std::collections::HashMap;
use std::fs;
use std::io::{Read as IoRead, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use memmap2::Mmap;
use rayon::prelude::*;

use crate::time::unix_now;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::collection::Collection;
use crate::error::FerresError;
use crate::point::Point;

// ─── TieredStorageConfig ─────────────────────────────────────────────

/// Configuração de tiered storage.
///
/// Quando habilitado, pontos são automaticamente movidos entre camadas
/// de armazenamento baseado na frequência de acesso.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredStorageConfig {
    /// Habilitado? (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Pontos acessados nas últimas N horas ficam em HOT (RAM).
    /// Default: 24 horas.
    #[serde(default = "default_hot_threshold")]
    pub hot_threshold_hours: u64,
    /// Pontos acessados nas últimas N horas ficam em WARM (mmap).
    /// Default: 168 horas (7 dias).
    #[serde(default = "default_warm_threshold")]
    pub warm_threshold_hours: u64,
    /// Abaixo disso: COLD (disco, carregado on-demand).
    /// Intervalo de compactação em segundos. Default: 3600 (1 hora).
    #[serde(default = "default_compaction_interval")]
    pub compaction_interval_secs: u64,
}

fn default_hot_threshold() -> u64 {
    24
}

fn default_warm_threshold() -> u64 {
    168
}

fn default_compaction_interval() -> u64 {
    3600
}

impl Default for TieredStorageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hot_threshold_hours: default_hot_threshold(),
            warm_threshold_hours: default_warm_threshold(),
            compaction_interval_secs: default_compaction_interval(),
        }
    }
}

// ─── StorageTier ──────────────────────────────────────────────────────

/// Camada de armazenamento de um ponto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageTier {
    /// Em RAM, acesso instantâneo. Ponto completo em memória.
    Hot,
    /// Memory-mapped, acesso rápido. Vetor em mmap, metadata em memória.
    Warm,
    /// Em disco, carregado sob demanda. Apenas ID em memória.
    Cold,
}

impl std::fmt::Display for StorageTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageTier::Hot => write!(f, "hot"),
            StorageTier::Warm => write!(f, "warm"),
            StorageTier::Cold => write!(f, "cold"),
        }
    }
}

// ─── AccessTracker ────────────────────────────────────────────────────

/// Rastreia acessos por ponto para decisão de tier.
///
/// Mantém o último timestamp de acesso e um contador para cada ponto.
/// Usado pelo compactador para decidir promoção/demoção.
#[derive(Debug)]
pub struct AccessTracker {
    /// point_id -> último acesso (unix timestamp em segundos).
    last_access: HashMap<String, u64>,
    /// point_id -> contador de acessos totais.
    access_count: HashMap<String, u64>,
}

impl AccessTracker {
    /// Cria um tracker vazio.
    pub fn new() -> Self {
        Self {
            last_access: HashMap::new(),
            access_count: HashMap::new(),
        }
    }

    /// Registra um acesso ao ponto.
    pub fn record_access(&mut self, id: &str) {
        let now = unix_now();
        self.last_access.insert(id.to_string(), now);
        *self.access_count.entry(id.to_string()).or_insert(0) += 1;
    }

    /// Registra acesso com timestamp customizado (útil para testes).
    pub fn record_access_at(&mut self, id: &str, timestamp: u64) {
        self.last_access.insert(id.to_string(), timestamp);
        *self.access_count.entry(id.to_string()).or_insert(0) += 1;
    }

    /// Determina o tier ideal para um ponto baseado no último acesso.
    pub fn get_tier(&self, id: &str, config: &TieredStorageConfig) -> StorageTier {
        let now = unix_now();
        self.get_tier_at(id, config, now)
    }

    /// Determina o tier ideal para um ponto com timestamp customizado.
    pub fn get_tier_at(&self, id: &str, config: &TieredStorageConfig, now: u64) -> StorageTier {
        let last = match self.last_access.get(id) {
            Some(&ts) => ts,
            None => return StorageTier::Cold, // Nunca acessado → Cold
        };

        let hours_since = now.saturating_sub(last) / 3600;

        if hours_since < config.hot_threshold_hours {
            StorageTier::Hot
        } else if hours_since < config.warm_threshold_hours {
            StorageTier::Warm
        } else {
            StorageTier::Cold
        }
    }

    /// Retorna pontos que devem ser promovidos ou demovidos.
    ///
    /// Compara o tier ideal (baseado no último acesso) com o tier atual
    /// de cada ponto. Retorna uma lista de (point_id, novo_tier).
    pub fn points_to_demote(
        &self,
        config: &TieredStorageConfig,
        current_tiers: &HashMap<String, StorageTier>,
    ) -> Vec<(String, StorageTier)> {
        let now = unix_now();
        self.points_to_demote_at(config, current_tiers, now)
    }

    /// Versão com timestamp customizado (para testes).
    pub fn points_to_demote_at(
        &self,
        config: &TieredStorageConfig,
        current_tiers: &HashMap<String, StorageTier>,
        now: u64,
    ) -> Vec<(String, StorageTier)> {
        let mut changes = Vec::new();

        for (id, current_tier) in current_tiers {
            let ideal_tier = self.get_tier_at(id, config, now);
            if *current_tier != ideal_tier {
                // Só demove (Hot→Warm, Warm→Cold, Hot→Cold)
                // Promoção acontece no acesso
                let should_change = matches!(
                    (&current_tier, &ideal_tier),
                    (StorageTier::Hot, StorageTier::Warm)
                        | (StorageTier::Hot, StorageTier::Cold)
                        | (StorageTier::Warm, StorageTier::Cold)
                );
                if should_change {
                    changes.push((id.clone(), ideal_tier));
                }
            }
        }

        changes
    }

    /// Último timestamp de acesso de um ponto (None se nunca acessado).
    pub fn last_access(&self, id: &str) -> Option<u64> {
        self.last_access.get(id).copied()
    }

    /// Contador de acessos de um ponto.
    pub fn access_count(&self, id: &str) -> u64 {
        self.access_count.get(id).copied().unwrap_or(0)
    }

    /// Remove tracking de um ponto (ao deletar).
    pub fn remove(&mut self, id: &str) {
        self.last_access.remove(id);
        self.access_count.remove(id);
    }

    /// Snapshot de last_access para uso em compactação paralela (sem manter o lock).
    pub fn last_access_snapshot(&self) -> HashMap<String, u64> {
        self.last_access.clone()
    }
}

impl Default for AccessTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── TierDistribution ─────────────────────────────────────────────────

/// Distribuição de pontos por tier com estimativa de memória.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierDistribution {
    /// Número de pontos na camada Hot (RAM).
    pub hot: usize,
    /// Número de pontos na camada Warm (mmap).
    pub warm: usize,
    /// Número de pontos na camada Cold (disco).
    pub cold: usize,
    /// Memória estimada usada pela camada Hot (bytes).
    pub hot_memory_bytes: usize,
    /// Memória estimada usada pela camada Warm (bytes).
    pub warm_memory_bytes: usize,
    /// Memória estimada usada pela camada Cold (bytes).
    pub cold_memory_bytes: usize,
}

// ─── WarmStorage ──────────────────────────────────────────────────────

/// Armazena vetores em arquivo memory-mapped para acesso rápido sem ocupar heap.
///
/// Formato do arquivo:
/// - 8 bytes: número de dimensões (u64 LE)
/// - Para cada vetor: dimension × 4 bytes (f32 LE)
///
/// Um arquivo de índice separado mapeia point_id → offset.
pub struct WarmStorage {
    /// Caminho do arquivo de vetores.
    vectors_path: PathBuf,
    /// Caminho do arquivo de índice (point_id → offset).
    index_path: PathBuf,
    /// Memory-map do arquivo de vetores (None se vazio).
    mmap: Option<Mmap>,
    /// Mapeamento point_id → offset no arquivo mmap.
    offsets: HashMap<String, usize>,
    /// Dimensão dos vetores.
    dimension: usize,
}

impl WarmStorage {
    /// Cria ou abre um WarmStorage no diretório especificado.
    pub fn new(dir: &Path, dimension: usize) -> Result<Self, FerresError> {
        fs::create_dir_all(dir)
            .map_err(|e| FerresError::Storage(format!("failed to create warm dir: {e}")))?;

        let vectors_path = dir.join("warm_vectors.bin");
        let index_path = dir.join("warm_index.json");

        let mut storage = Self {
            vectors_path,
            index_path,
            mmap: None,
            offsets: HashMap::new(),
            dimension,
        };

        // Carrega índice existente se houver
        if storage.index_path.exists() {
            let index_data = fs::read_to_string(&storage.index_path)
                .map_err(|e| FerresError::Storage(format!("failed to read warm index: {e}")))?;
            storage.offsets = serde_json::from_str(&index_data)
                .map_err(|e| FerresError::Storage(format!("failed to parse warm index: {e}")))?;
        }

        // Carrega mmap com verificação de consistência (crash recovery)
        storage.reload_mmap()?;

        Ok(storage)
    }

    /// Adiciona um vetor ao armazenamento warm.
    pub fn add_vector(&mut self, id: &str, vector: &[f32]) -> Result<(), FerresError> {
        // Calcula o offset no final do arquivo (antes de inserir no mapa)
        let offset = self.offsets.len() * self.dimension * 4;

        // Escreve o vetor no arquivo e faz fsync antes de atualizar estado em memória
        {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.vectors_path)
                .map_err(|e| FerresError::Storage(format!("failed to open warm vectors: {e}")))?;

            for &val in vector {
                file.write_all(&val.to_le_bytes()).map_err(|e| {
                    FerresError::Storage(format!("failed to write warm vector: {e}"))
                })?;
            }

            // fsync garante que os bytes estão no disco antes de atualizar o índice
            file.sync_all()
                .map_err(|e| FerresError::Storage(format!("failed to sync warm vectors: {e}")))?;
        } // file handle é fechado aqui

        // Agora é seguro atualizar o mapa de offsets
        self.offsets.insert(id.to_string(), offset);

        // Atualiza o índice (escrita atômica: tmp + sync + rename)
        self.save_index()?;

        // Re-mmap
        self.reload_mmap()?;

        Ok(())
    }

    /// Lê um vetor do armazenamento warm.
    pub fn read_vector(&self, id: &str) -> Result<Vec<f32>, FerresError> {
        let offset = self.offsets.get(id).ok_or_else(|| {
            FerresError::Storage(format!("vector '{id}' not found in warm storage"))
        })?;

        let mmap = self
            .mmap
            .as_ref()
            .ok_or_else(|| FerresError::Storage("warm storage mmap not initialized".to_string()))?;

        let byte_offset = *offset;
        let byte_len = self.dimension * 4;

        if byte_offset + byte_len > mmap.len() {
            return Err(FerresError::Storage(format!(
                "warm vector offset out of bounds for '{id}'"
            )));
        }

        let bytes = &mmap[byte_offset..byte_offset + byte_len];
        let vector: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| {
                let arr: [u8; 4] = chunk
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("chunks_exact(4) guarantees a 4-byte slice"));
                f32::from_le_bytes(arr)
            })
            .collect();

        Ok(vector)
    }

    /// Remove um vetor do armazenamento warm.
    ///
    /// Nota: não remove do arquivo mmap (fragmentação é aceita),
    /// apenas remove do índice. A próxima reconstrução limpa.
    pub fn remove_vector(&mut self, id: &str) {
        self.offsets.remove(id);
    }

    /// Verifica se um vetor existe no warm storage.
    pub fn contains(&self, id: &str) -> bool {
        self.offsets.contains_key(id)
    }

    /// Número de vetores no warm storage.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Retorna true se não há vetores no warm storage.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Força sync de ambos os arquivos (vetores e índice) para disco.
    ///
    /// Garante que todos os dados escritos até o momento estão persistidos
    /// de forma durável. Útil antes de confirmações críticas.
    pub fn flush(&mut self) -> Result<(), FerresError> {
        // Soltar o mmap antes de sync (necessário no Windows)
        self.mmap = None;

        // Sync do arquivo de vetores (requer write access para FlushFileBuffers)
        if self.vectors_path.exists() {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(&self.vectors_path)
                .map_err(|e| {
                    FerresError::Storage(format!("failed to open warm vectors for flush: {e}"))
                })?;
            file.sync_all()
                .map_err(|e| FerresError::Storage(format!("failed to flush warm vectors: {e}")))?;
        }

        // Escrita atômica do índice (tmp + sync + rename)
        self.save_index()?;

        // Recriar o mmap
        self.reload_mmap()?;

        Ok(())
    }

    /// Reconstrói o arquivo mmap com apenas os vetores ativos.
    pub fn compact(&mut self, vectors: &HashMap<String, Vec<f32>>) -> Result<(), FerresError> {
        // Reescreve o arquivo com apenas os vetores ativos
        let tmp_path = self.vectors_path.with_extension("tmp");
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| FerresError::Storage(format!("failed to create temp warm file: {e}")))?;

        let mut new_offsets = HashMap::new();
        let mut offset = 0;

        for (id, vector) in vectors {
            new_offsets.insert(id.clone(), offset);
            for &val in vector {
                file.write_all(&val.to_le_bytes()).map_err(|e| {
                    FerresError::Storage(format!("failed to write warm vector: {e}"))
                })?;
            }
            offset += vector.len() * 4;
        }

        // Sync antes de rename para garantir integridade
        file.sync_all().map_err(|e| {
            FerresError::Storage(format!("failed to sync warm vectors during compact: {e}"))
        })?;

        // Atomic rename
        fs::rename(&tmp_path, &self.vectors_path)
            .map_err(|e| FerresError::Storage(format!("failed to rename warm vectors: {e}")))?;

        self.offsets = new_offsets;
        self.save_index()?;
        self.reload_mmap()?;

        Ok(())
    }

    fn save_index(&self) -> Result<(), FerresError> {
        let index_json = serde_json::to_string(&self.offsets)
            .map_err(|e| FerresError::Storage(format!("failed to serialize warm index: {e}")))?;

        // Escrita atômica: warm_index.json.tmp → sync_all → rename → warm_index.json
        let tmp_path = PathBuf::from(format!("{}.tmp", self.index_path.display()));

        let mut tmp_file = fs::File::create(&tmp_path)
            .map_err(|e| FerresError::Storage(format!("failed to create warm index tmp: {e}")))?;
        tmp_file
            .write_all(index_json.as_bytes())
            .map_err(|e| FerresError::Storage(format!("failed to write warm index: {e}")))?;
        tmp_file
            .sync_all()
            .map_err(|e| FerresError::Storage(format!("failed to sync warm index: {e}")))?;

        fs::rename(&tmp_path, &self.index_path)
            .map_err(|e| FerresError::Storage(format!("failed to rename warm index: {e}")))?;
        Ok(())
    }

    fn reload_mmap(&mut self) -> Result<(), FerresError> {
        if !self.vectors_path.exists() {
            self.mmap = None;
            return Ok(());
        }

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.vectors_path)
            .map_err(|e| FerresError::Storage(format!("failed to open warm vectors: {e}")))?;

        let file_len = file
            .metadata()
            .map_err(|e| FerresError::Storage(format!("failed to get metadata: {e}")))?
            .len() as usize;

        // ── Crash recovery: verificar consistência entre índice e arquivo ──
        let vector_bytes = self.dimension * 4;
        let before_count = self.offsets.len();

        // Remove entradas cujo vetor extrapola o tamanho real do arquivo
        self.offsets
            .retain(|_, offset| *offset + vector_bytes <= file_len);

        if self.offsets.len() < before_count {
            warn!(
                removed = before_count - self.offsets.len(),
                kept = self.offsets.len(),
                file_len = file_len,
                "warm storage crash recovery: removed entries beyond file bounds"
            );
        }

        // Último byte válido segundo o índice
        let max_valid_end = self
            .offsets
            .values()
            .map(|&offset| offset + vector_bytes)
            .max()
            .unwrap_or(0);

        // Truncar lixo residual (escrita parcial que não chegou ao índice)
        let needs_truncate = file_len > max_valid_end && max_valid_end > 0;
        let lost_entries = self.offsets.len() < before_count;

        if needs_truncate {
            file.set_len(max_valid_end as u64).map_err(|e| {
                FerresError::Storage(format!("failed to truncate warm vectors: {e}"))
            })?;
            file.sync_all()
                .map_err(|e| FerresError::Storage(format!("failed to sync after truncate: {e}")))?;
        }

        if lost_entries || needs_truncate {
            self.save_index()?;
        }

        // Mapear o arquivo (pode ter sido truncado)
        let final_len = if needs_truncate {
            max_valid_end as u64
        } else {
            file_len as u64
        };

        if final_len > 0 {
            let mmap = unsafe {
                Mmap::map(&file).map_err(|e| {
                    FerresError::Storage(format!("failed to mmap warm vectors: {e}"))
                })?
            };
            self.mmap = Some(mmap);
        } else {
            self.mmap = None;
        }

        Ok(())
    }
}

// ─── ColdStorage ──────────────────────────────────────────────────────

/// Armazena pontos completos em disco, carregados sob demanda.
///
/// Cada ponto é serializado como JSON em um arquivo individual
/// em `<dir>/cold/<point_id>.json`.
///
/// # Migração (v0.x → percent-encoding)
///
/// Versões anteriores usavam `str::replace` para sanitizar IDs, substituindo
/// caracteres especiais por `_`. Isso causava colisões silenciosas — por exemplo,
/// `"doc/1"` e `"doc_1"` mapeavam para o mesmo arquivo `doc_1.json`.
///
/// A partir desta versão, `point_path` usa percent-encoding (e.g. `"doc/1"` →
/// `"doc%2F1.json"`), eliminando colisões. Dados salvos com a versão anterior
/// **não** são migrados automaticamente e podem não ser encontrados com o novo
/// esquema de nomes. Como pontos cold podem ser reconstruídos promovendo de volta
/// e re-demovendo, a perda é aceitável. Uma migração futura poderia varrer o
/// diretório `cold/` e renomear arquivos conforme necessário.
pub struct ColdStorage {
    dir: PathBuf,
}

impl ColdStorage {
    /// Cria ou abre um ColdStorage no diretório especificado.
    pub fn new(dir: &Path) -> Result<Self, FerresError> {
        let cold_dir = dir.join("cold");
        fs::create_dir_all(&cold_dir)
            .map_err(|e| FerresError::Storage(format!("failed to create cold dir: {e}")))?;
        Ok(Self { dir: cold_dir })
    }

    /// Salva um ponto completo em disco.
    pub fn save_point(&self, point: &Point) -> Result<(), FerresError> {
        let path = self.point_path(&point.id);
        let json = serde_json::to_string(point)
            .map_err(|e| FerresError::Storage(format!("failed to serialize cold point: {e}")))?;
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &json)
            .map_err(|e| FerresError::Storage(format!("failed to write cold point: {e}")))?;
        fs::rename(&tmp_path, &path)
            .map_err(|e| FerresError::Storage(format!("failed to rename cold point: {e}")))?;
        Ok(())
    }

    /// Carrega um ponto completo do disco.
    pub fn load_point(&self, id: &str) -> Result<Point, FerresError> {
        let path = self.point_path(id);
        let mut file = fs::File::open(&path)
            .map_err(|e| FerresError::Storage(format!("failed to open cold point '{id}': {e}")))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| FerresError::Storage(format!("failed to read cold point '{id}': {e}")))?;
        let point: Point = serde_json::from_str(&content)
            .map_err(|e| FerresError::Storage(format!("failed to parse cold point '{id}': {e}")))?;
        Ok(point)
    }

    /// Remove um ponto do disco.
    pub fn remove_point(&self, id: &str) {
        let path = self.point_path(id);
        let _ = fs::remove_file(&path);
    }

    /// Verifica se um ponto existe no cold storage.
    pub fn contains(&self, id: &str) -> bool {
        self.point_path(id).exists()
    }

    fn point_path(&self, id: &str) -> PathBuf {
        // Percent-encoding para evitar colisões entre IDs distintos.
        // Caracteres seguros (alfanuméricos, '-', '_', '.') são mantidos;
        // todos os outros são codificados como %XX por byte UTF-8.
        let mut safe = String::with_capacity(id.len() * 2);
        for c in id.chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                safe.push(c);
            } else {
                // percent-encode: '/' -> "%2F", ':' -> "%3A", etc.
                for byte in c.to_string().as_bytes() {
                    use std::fmt::Write;
                    let _ = write!(&mut safe, "%{byte:02X}");
                }
            }
        }
        self.dir.join(format!("{safe}.json"))
    }
}

// ─── Helpers para compactação paralela ─────────────────────────────────

/// Calcula o tier ideal a partir do último acesso (para uso em rayon sem lock).
fn get_tier_at_from_last_access(
    last_ts: Option<u64>,
    config: &TieredStorageConfig,
    now: u64,
) -> StorageTier {
    let last = match last_ts {
        Some(ts) => ts,
        None => return StorageTier::Cold,
    };
    let hours_since = now.saturating_sub(last) / 3600;
    if hours_since < config.hot_threshold_hours {
        StorageTier::Hot
    } else if hours_since < config.warm_threshold_hours {
        StorageTier::Warm
    } else {
        StorageTier::Cold
    }
}

/// Indica se o ponto deve ser demovido (Hot→Warm, Hot→Cold, Warm→Cold).
fn should_demote(current: &StorageTier, ideal: &StorageTier) -> bool {
    matches!(
        (current, ideal),
        (StorageTier::Hot, StorageTier::Warm)
            | (StorageTier::Hot, StorageTier::Cold)
            | (StorageTier::Warm, StorageTier::Cold)
    )
}

// ─── TieredCollection ─────────────────────────────────────────────────

/// Wrapper sobre `Collection` que adiciona tiered storage.
///
/// - **HOT**: pontos completos em memória (como a Collection padrão).
/// - **WARM**: vetores em mmap, metadata em memória (HashMap separado).
/// - **COLD**: apenas IDs em memória, dados em disco (carregado on-demand).
///
/// O grafo HNSW **sempre** fica em memória, independente do tier.
pub struct TieredCollection {
    /// Coleção interna (contém os pontos HOT e o índice HNSW).
    collection: Collection,
    /// Configuração de tiered storage.
    config: TieredStorageConfig,
    /// Tracker de acessos por ponto.
    access_tracker: Mutex<AccessTracker>,
    /// Tier atual de cada ponto.
    point_tiers: RwLock<HashMap<String, StorageTier>>,
    /// Warm storage (vetores em mmap).
    warm_storage: Option<Mutex<WarmStorage>>,
    /// Cold storage (pontos em disco).
    cold_storage: Option<ColdStorage>,
    /// Metadata de pontos warm (tudo menos o vetor, que está no mmap).
    warm_metadata: RwLock<HashMap<String, WarmPointMeta>>,
    /// IDs de pontos cold (dados estão no disco).
    cold_ids: RwLock<HashMap<String, ColdPointMeta>>,
}

/// Metadata de um ponto na camada Warm (vetor no mmap, resto em memória).
#[derive(Debug, Clone)]
struct WarmPointMeta {
    metadata: serde_json::Value,
    created_at: u64,
    expires_at: Option<u64>,
}

/// Metadata mínima de um ponto na camada Cold (dados no disco).
#[derive(Debug, Clone)]
struct ColdPointMeta {
    #[allow(dead_code)]
    created_at: u64,
    #[allow(dead_code)]
    expires_at: Option<u64>,
}

impl TieredCollection {
    /// Cria uma TieredCollection a partir de uma Collection existente.
    ///
    /// Se tiered storage estiver desabilitado, todos os pontos ficam em HOT.
    pub fn new(
        collection: Collection,
        tiered_config: TieredStorageConfig,
        storage_dir: Option<&Path>,
    ) -> Result<Self, FerresError> {
        let dimension = collection.config().dimension;

        // Inicializa warm/cold storage se habilitado
        let (warm_storage, cold_storage) = if tiered_config.enabled {
            if let Some(dir) = storage_dir {
                let warm = WarmStorage::new(&dir.join("warm"), dimension)?;
                let cold = ColdStorage::new(dir)?;
                (Some(Mutex::new(warm)), Some(cold))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Todos os pontos existentes começam como HOT
        let mut point_tiers = HashMap::new();
        let mut tracker = AccessTracker::new();
        let now = unix_now();

        for point in collection.points_owned() {
            point_tiers.insert(point.id.clone(), StorageTier::Hot);
            tracker.record_access_at(&point.id, now);
        }

        Ok(Self {
            collection,
            config: tiered_config,
            access_tracker: Mutex::new(tracker),
            point_tiers: RwLock::new(point_tiers),
            warm_storage,
            cold_storage,
            warm_metadata: RwLock::new(HashMap::new()),
            cold_ids: RwLock::new(HashMap::new()),
        })
    }

    /// Insere um ponto (sempre começa como HOT).
    pub fn insert(&mut self, point: Point) -> Result<(), FerresError> {
        let id = point.id.clone();
        self.collection.insert(point)?;

        // Marca como HOT e registra acesso
        if let Ok(mut tiers) = self.point_tiers.write() {
            tiers.insert(id.clone(), StorageTier::Hot);
        }
        if let Ok(mut tracker) = self.access_tracker.lock() {
            tracker.record_access(&id);
        }

        Ok(())
    }

    /// Remove um ponto de todas as camadas.
    pub fn remove(&mut self, id: &str) -> Result<(), FerresError> {
        // Tenta remover da Collection (HOT)
        let _ = self.collection.remove(id);

        // Remove de Warm
        if let Some(ref warm) = self.warm_storage {
            if let Ok(mut ws) = warm.lock() {
                ws.remove_vector(id);
            }
        }
        if let Ok(mut wm) = self.warm_metadata.write() {
            wm.remove(id);
        }

        // Remove de Cold
        if let Some(ref cold) = self.cold_storage {
            cold.remove_point(id);
        }
        if let Ok(mut ci) = self.cold_ids.write() {
            ci.remove(id);
        }

        // Remove dos tiers e tracker
        if let Ok(mut tiers) = self.point_tiers.write() {
            tiers.remove(id);
        }
        if let Ok(mut tracker) = self.access_tracker.lock() {
            tracker.remove(id);
        }

        Ok(())
    }

    /// Busca os k vizinhos mais próximos.
    ///
    /// O HNSW busca normalmente (grafo completo em memória).
    /// Hidratação: hot é instantâneo, warm lê do mmap, cold lê do disco.
    /// Registra acesso para cada resultado retornado.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>, FerresError> {
        let results = self.collection.search(query, k, None, None)?;

        // Registra acesso para cada resultado
        if let Ok(mut tracker) = self.access_tracker.lock() {
            for (id, _) in &results {
                tracker.record_access(id);
            }
        }

        Ok(results)
    }

    /// Recupera um ponto completo pelo ID, promovendo automaticamente.
    ///
    /// - Hot: retorna imediatamente da memória.
    /// - Warm: lê vetor do mmap + metadata da memória, promove para Hot.
    /// - Cold: lê do disco, promove para Hot.
    pub fn get(&self, id: &str) -> Result<Option<Point>, FerresError> {
        // Lock order: point_tiers first, then access_tracker (same as insert/remove)
        let tier = self
            .point_tiers
            .read()
            .map_err(|_| FerresError::Storage("failed to read point tiers".into()))?
            .get(id)
            .cloned();

        if let Ok(mut tracker) = self.access_tracker.lock() {
            tracker.record_access(id);
        }

        match tier {
            Some(StorageTier::Hot) | None => {
                // Tenta buscar na Collection (HOT)
                Ok(self.collection.get(id).cloned())
            }
            Some(StorageTier::Warm) => {
                // Lê do warm storage
                self.hydrate_from_warm(id)
            }
            Some(StorageTier::Cold) => {
                // Lê do cold storage
                self.hydrate_from_cold(id)
            }
        }
    }

    /// Hidrata um ponto do warm storage (promove para Hot).
    fn hydrate_from_warm(&self, id: &str) -> Result<Option<Point>, FerresError> {
        let vector = if let Some(ref warm) = self.warm_storage {
            let ws = warm
                .lock()
                .map_err(|_| FerresError::Storage("failed to lock warm storage".into()))?;
            if ws.contains(id) {
                Some(ws.read_vector(id)?)
            } else {
                None
            }
        } else {
            None
        };

        let meta = self
            .warm_metadata
            .read()
            .map_err(|_| FerresError::Storage("failed to read warm metadata".into()))?
            .get(id)
            .cloned();

        match (vector, meta) {
            (Some(vec), Some(m)) => {
                let (namespace, logical_id) = Point::parse_storage_id(id);
                let point = Point {
                    id: logical_id,
                    vector: vec,
                    metadata: m.metadata,
                    created_at: m.created_at,
                    namespace,
                    expires_at: m.expires_at,
                    vectors: None,
                    relations: None,
                };
                Ok(Some(point))
            }
            _ => Ok(None),
        }
    }

    /// Hidrata um ponto do cold storage (promove para Hot).
    fn hydrate_from_cold(&self, id: &str) -> Result<Option<Point>, FerresError> {
        if let Some(ref cold) = self.cold_storage {
            match cold.load_point(id) {
                Ok(point) => Ok(Some(point)),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Promove um ponto para Hot (chamado automaticamente no acesso).
    ///
    /// Hidrata o ponto do tier atual (Warm ou Cold), tombstona o nó antigo
    /// no HNSW para evitar duplicatas, e re-insere na Collection.
    pub fn promote_to_hot(&mut self, id: &str) -> Result<(), FerresError> {
        let current_tier = self
            .point_tiers
            .read()
            .map_err(|_| FerresError::Storage("failed to read tiers".into()))?
            .get(id)
            .cloned();

        match current_tier {
            Some(StorageTier::Warm) => {
                // Lê do warm e insere na collection
                if let Ok(Some(point)) = self.hydrate_from_warm(id) {
                    // Tombstona o nó antigo no HNSW antes de re-inserir,
                    // evitando duplicatas no grafo.
                    self.collection.tombstone_in_index(id);
                    self.collection.insert(point)?;
                    // Remove do warm
                    if let Some(ref warm) = self.warm_storage {
                        if let Ok(mut ws) = warm.lock() {
                            ws.remove_vector(id);
                        }
                    }
                    if let Ok(mut wm) = self.warm_metadata.write() {
                        wm.remove(id);
                    }
                    if let Ok(mut tiers) = self.point_tiers.write() {
                        tiers.insert(id.to_string(), StorageTier::Hot);
                    }
                }
            }
            Some(StorageTier::Cold) => {
                // Lê do cold e insere na collection
                if let Ok(Some(point)) = self.hydrate_from_cold(id) {
                    // Tombstona o nó antigo no HNSW antes de re-inserir
                    self.collection.tombstone_in_index(id);
                    self.collection.insert(point)?;
                    // Remove do cold
                    if let Some(ref cold) = self.cold_storage {
                        cold.remove_point(id);
                    }
                    if let Ok(mut ci) = self.cold_ids.write() {
                        ci.remove(id);
                    }
                    if let Ok(mut tiers) = self.point_tiers.write() {
                        tiers.insert(id.to_string(), StorageTier::Hot);
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Demove um ponto de Hot para Warm.
    ///
    /// O vetor é movido para mmap (WarmStorage) e a metadata permanece em
    /// memória (HashMap separado). O ponto é removido do HashMap da Collection
    /// para liberar RAM, mas **NÃO** é tombstonado no HNSW — o nó permanece
    /// ativo no grafo para que buscas continuem retornando este ponto.
    ///
    /// A hidratação posterior via [`get_from_any_tier`] reconstrói o ponto
    /// a partir do warm storage.
    pub fn demote_to_warm(&mut self, id: &str) -> Result<(), FerresError> {
        // Busca o ponto na collection
        let point = self.collection.get(id).cloned();
        if let Some(point) = point {
            // Escreve vetor no warm storage
            if let Some(ref warm) = self.warm_storage {
                let mut ws = warm
                    .lock()
                    .map_err(|_| FerresError::Storage("failed to lock warm storage".into()))?;
                ws.add_vector(id, &point.vector)?;
            }

            // Guarda metadata em memória
            if let Ok(mut wm) = self.warm_metadata.write() {
                wm.insert(
                    id.to_string(),
                    WarmPointMeta {
                        metadata: point.metadata.clone(),
                        created_at: point.created_at,
                        expires_at: point.expires_at,
                    },
                );
            }

            // Remove dados da Collection (libera Vec<f32> da RAM) mas NÃO
            // tombstona no HNSW — o nó permanece no grafo para buscas.
            let _ = self.collection.remove_data_only(id);

            // Atualiza tier
            if let Ok(mut tiers) = self.point_tiers.write() {
                tiers.insert(id.to_string(), StorageTier::Warm);
            }

            debug!(point_id = id, "demoted point to warm tier");
        }

        Ok(())
    }

    /// Demove um ponto de Warm para Cold.
    pub fn demote_to_cold(&mut self, id: &str) -> Result<(), FerresError> {
        // Lê do warm
        let point = self.hydrate_from_warm(id)?;
        if let Some(point) = point {
            // Salva em cold
            if let Some(ref cold) = self.cold_storage {
                cold.save_point(&point)?;
            }

            // Remove do warm
            if let Some(ref warm) = self.warm_storage {
                if let Ok(mut ws) = warm.lock() {
                    ws.remove_vector(id);
                }
            }
            if let Ok(mut wm) = self.warm_metadata.write() {
                wm.remove(id);
            }

            // Guarda metadata mínima
            if let Ok(mut ci) = self.cold_ids.write() {
                ci.insert(
                    id.to_string(),
                    ColdPointMeta {
                        created_at: point.created_at,
                        expires_at: point.expires_at,
                    },
                );
            }

            // Atualiza tier
            if let Ok(mut tiers) = self.point_tiers.write() {
                tiers.insert(id.to_string(), StorageTier::Cold);
            }

            debug!(point_id = id, "demoted point to cold tier");
        }

        Ok(())
    }

    /// Processa em batch todos os pontos que devem ir para Warm (um único lock no warm_storage).
    fn demote_batch_to_warm(&mut self, ids: &[String]) -> Result<usize, FerresError> {
        if ids.is_empty() {
            return Ok(0);
        }
        // Coleta pontos da collection (sem segurar warm lock)
        let points: Vec<(String, Point)> = ids
            .iter()
            .filter_map(|id| self.collection.get(id).cloned().map(|p| (id.clone(), p)))
            .collect();
        let count = points.len();

        // Um único lock: escreve todos os vetores no warm
        if let Some(ref warm) = self.warm_storage {
            let mut ws = warm
                .lock()
                .map_err(|_| FerresError::Storage("failed to lock warm storage".into()))?;
            for (id, ref point) in &points {
                ws.add_vector(id, &point.vector)?;
            }
        }

        // Atualiza metadata, remove da collection e atualiza tiers
        for (id, point) in &points {
            if let Ok(mut wm) = self.warm_metadata.write() {
                wm.insert(
                    id.clone(),
                    WarmPointMeta {
                        metadata: point.metadata.clone(),
                        created_at: point.created_at,
                        expires_at: point.expires_at,
                    },
                );
            }
            let _ = self.collection.remove_data_only(id);
            if let Ok(mut tiers) = self.point_tiers.write() {
                tiers.insert(id.clone(), StorageTier::Warm);
            }
            debug!(point_id = id, "demoted point to warm tier");
        }

        Ok(count)
    }

    /// Processa em batch todos os pontos que devem ir para Cold (um único lock no warm para remoções).
    fn demote_batch_to_cold(&mut self, ids: &[String]) -> Result<usize, FerresError> {
        if ids.is_empty() {
            return Ok(0);
        }
        // Lê metadata warm (read lock) para todos os ids
        let meta_map: HashMap<String, WarmPointMeta> = {
            let wm = self
                .warm_metadata
                .read()
                .map_err(|_| FerresError::Storage("failed to read warm metadata".into()))?;
            ids.iter()
                .filter_map(|id| wm.get(id).cloned().map(|m| (id.clone(), m)))
                .collect()
        };

        // Um único lock no warm: lê vetores e remove todos
        let points: Vec<(String, Point)> = if let Some(ref warm) = self.warm_storage {
            let mut ws = warm
                .lock()
                .map_err(|_| FerresError::Storage("failed to lock warm storage".into()))?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let (Some(vec), Some(meta)) = (
                    ws.contains(id)
                        .then(|| ws.read_vector(id))
                        .and_then(|r| r.ok()),
                    meta_map.get(id),
                ) {
                    let (namespace, logical_id) = Point::parse_storage_id(id);
                    out.push((
                        id.clone(),
                        Point {
                            id: logical_id,
                            vector: vec,
                            metadata: meta.metadata.clone(),
                            created_at: meta.created_at,
                            namespace,
                            expires_at: meta.expires_at,
                            vectors: None,
                            relations: None,
                        },
                    ));
                }
            }
            for (id, _) in &out {
                ws.remove_vector(id);
            }
            out
        } else {
            Vec::new()
        };

        // Remove metadata warm e persiste em cold
        for (id, ref point) in &points {
            if let Ok(mut wm) = self.warm_metadata.write() {
                wm.remove(id);
            }
            if let Some(ref cold) = self.cold_storage {
                cold.save_point(point)?;
            }
            if let Ok(mut ci) = self.cold_ids.write() {
                ci.insert(
                    id.clone(),
                    ColdPointMeta {
                        created_at: point.created_at,
                        expires_at: point.expires_at,
                    },
                );
            }
            if let Ok(mut tiers) = self.point_tiers.write() {
                tiers.insert(id.clone(), StorageTier::Cold);
            }
            debug!(point_id = id, "demoted point to cold tier");
        }

        Ok(points.len())
    }

    /// Roda a compactação: demove pontos baseado no AccessTracker.
    ///
    /// Chamado periodicamente pelo background task.
    /// Cálculo de points_to_demote é paralelizado com rayon; writes são em batch por tier.
    pub fn run_compaction(&mut self) -> Result<CompactionResult, FerresError> {
        if !self.config.enabled {
            return Ok(CompactionResult::default());
        }

        let compaction_start = Instant::now();

        // Snapshot para cálculo paralelo (locks breves)
        let (tiers_vec, last_access_snapshot, config_clone) = {
            let tracker = self
                .access_tracker
                .lock()
                .map_err(|_| FerresError::Storage("failed to lock access tracker".into()))?;
            let tiers = self
                .point_tiers
                .read()
                .map_err(|_| FerresError::Storage("failed to read tiers".into()))?;
            let tiers_vec: Vec<(String, StorageTier)> =
                tiers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let last_access_snapshot = tracker.last_access_snapshot();
            (tiers_vec, last_access_snapshot, self.config.clone())
        };

        let now = unix_now();

        // Cálculo paralelo de points_to_demote
        let changes: Vec<(String, StorageTier)> = tiers_vec
            .par_iter()
            .filter_map(|(id, current_tier)| {
                let ideal = get_tier_at_from_last_access(
                    last_access_snapshot.get(id.as_str()).copied(),
                    &config_clone,
                    now,
                );
                if should_demote(current_tier, &ideal) {
                    Some((id.clone(), ideal))
                } else {
                    None
                }
            })
            .collect();

        // Snapshot do tier atual para decidir batches (Hot→Warm vs Hot→Cold vs Warm→Cold)
        let tiers_snapshot: HashMap<String, StorageTier> = self
            .point_tiers
            .read()
            .map_err(|_| FerresError::Storage("failed to read tiers".into()))?
            .clone();

        // IDs para demote_to_warm: (Hot→Warm) e (Hot→Cold, pois precisa Warm primeiro)
        let to_warm_ids: Vec<String> = changes
            .iter()
            .filter_map(|(id, new_tier)| {
                let current = tiers_snapshot.get(id)?;
                match (current, new_tier) {
                    (StorageTier::Hot, StorageTier::Warm) => Some(id.clone()),
                    (StorageTier::Hot, StorageTier::Cold) => Some(id.clone()),
                    _ => None,
                }
            })
            .collect();

        // IDs para demote_to_cold: todos que devem terminar em Cold (já Warm após batch warm)
        let to_cold_ids: Vec<String> = changes
            .iter()
            .filter_map(|(id, new_tier)| {
                if *new_tier == StorageTier::Cold {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        // Batch: primeiro todos para Warm, depois todos para Cold
        let demoted_to_warm = self.demote_batch_to_warm(&to_warm_ids)?;
        let demoted_to_cold = self.demote_batch_to_cold(&to_cold_ids)?;

        let compaction_ms = compaction_start.elapsed().as_millis();
        let result = CompactionResult {
            demoted_to_warm,
            demoted_to_cold,
            total_changes: changes.len(),
        };

        if result.total_changes > 0 {
            info!(
                compaction_ms,
                demoted_warm = demoted_to_warm,
                demoted_cold = demoted_to_cold,
                "tiered compaction completed"
            );
        }

        Ok(result)
    }

    /// Retorna a distribuição de pontos por tier.
    pub fn tier_distribution(&self) -> TierDistribution {
        let dimension = self.collection.config().dimension;
        let tiers = self.point_tiers.read().unwrap_or_else(|e| e.into_inner());

        let mut hot = 0usize;
        let mut warm = 0usize;
        let mut cold = 0usize;

        for tier in tiers.values() {
            match tier {
                StorageTier::Hot => hot += 1,
                StorageTier::Warm => warm += 1,
                StorageTier::Cold => cold += 1,
            }
        }

        // Estimativas de memória:
        // Hot: vetor f32 (dim*4) + metadata (~200 bytes) + HNSW node
        // Warm: metadata (~200 bytes) + mmap overhead (~64 bytes)
        // Cold: ID (~64 bytes)
        let vector_bytes = dimension * 4;
        let metadata_est = 200;
        let hnsw_node_est = 128;

        TierDistribution {
            hot,
            warm,
            cold,
            hot_memory_bytes: hot * (vector_bytes + metadata_est + hnsw_node_est),
            warm_memory_bytes: warm * (metadata_est + 64),
            cold_memory_bytes: cold * 64,
        }
    }

    /// Recupera um ponto de qualquer tier **sem promoção automática**.
    ///
    /// Diferente de [`get`], este método NÃO promove o ponto para Hot.
    /// É projetado para hidratação de resultados de busca, onde queremos
    /// os dados do ponto (especialmente metadata para filtros) mas sem
    /// adicionar latência de escrita (promoção) no caminho de leitura.
    ///
    /// Em vez disso, registra o acesso no [`AccessTracker`] para que o
    /// próximo ciclo de compactação possa promover pontos acessados com
    /// frequência (promoção lazy/implícita).
    ///
    /// # Performance
    ///
    /// | Tier | Latência | Operação |
    /// |------|----------|----------|
    /// | Hot  | ~0 µs    | HashMap lookup (RAM) |
    /// | Warm | ~1-10 µs | mmap read + HashMap lookup |
    /// | Cold | ~100+ µs | Disk I/O (JSON deserialization) |
    ///
    /// **Atenção**: carregar pontos Cold durante busca adiciona latência
    /// significativa. Para workloads com muitos pontos Cold nos resultados,
    /// considere ajustar `warm_threshold_hours` para manter mais pontos
    /// em Warm (mmap), que tem latência muito menor que disco.
    ///
    /// # Retorno
    ///
    /// `Some(Point)` se encontrado em qualquer tier, `None` se o ponto
    /// não existe em nenhum tier.
    pub fn get_from_any_tier(&self, id: &str) -> Option<Point> {
        // 1. Tenta Hot (in-memory, via Collection HashMap)
        if let Some(p) = self.collection.get(id) {
            return Some(p.clone());
        }

        // 2. Tenta Warm (mmap vector + in-memory metadata)
        if let Ok(Some(p)) = self.hydrate_from_warm(id) {
            // Registra acesso para promoção lazy no próximo ciclo de compactação
            if let Ok(mut tracker) = self.access_tracker.lock() {
                tracker.record_access(id);
            }
            return Some(p);
        }

        // 3. Tenta Cold (disk I/O)
        if let Ok(Some(p)) = self.hydrate_from_cold(id) {
            // Registra acesso para promoção lazy
            if let Ok(mut tracker) = self.access_tracker.lock() {
                tracker.record_access(id);
            }
            return Some(p);
        }

        None
    }

    /// Retorna referência à coleção interna.
    pub fn collection(&self) -> &Collection {
        &self.collection
    }

    /// Retorna referência mutável à coleção interna.
    pub fn collection_mut(&mut self) -> &mut Collection {
        &mut self.collection
    }

    /// Retorna a configuração de tiered storage.
    pub fn tiered_config(&self) -> &TieredStorageConfig {
        &self.config
    }

    /// Nome da coleção.
    pub fn name(&self) -> &str {
        self.collection.name()
    }

    /// Número total de pontos (todas as camadas).
    pub fn len(&self) -> usize {
        self.point_tiers.read().map(|t| t.len()).unwrap_or(0)
    }

    /// Verifica se está vazia.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Retorna o tier atual de um ponto.
    pub fn point_tier(&self, id: &str) -> Option<StorageTier> {
        self.point_tiers.read().ok()?.get(id).cloned()
    }
}

/// Resultado de uma operação de compactação.
#[derive(Debug, Default)]
pub struct CompactionResult {
    /// Pontos demovidos para Warm.
    pub demoted_to_warm: usize,
    /// Pontos demovidos para Cold.
    pub demoted_to_cold: usize,
    /// Total de mudanças.
    pub total_changes: usize,
}

// ─── TierMetadata (para persistência) ─────────────────────────────────

/// Metadados de tier para persistência em disco.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierMetadata {
    /// Tier atual de cada ponto.
    pub point_tiers: HashMap<String, StorageTier>,
    /// Último acesso de cada ponto (unix timestamp).
    pub last_access: HashMap<String, u64>,
    /// Contador de acessos de cada ponto.
    pub access_count: HashMap<String, u64>,
}

impl TierMetadata {
    /// Exporta metadados de uma TieredCollection.
    pub fn from_tiered_collection(tc: &TieredCollection) -> Self {
        let point_tiers = tc.point_tiers.read().map(|t| t.clone()).unwrap_or_default();

        let tracker = tc.access_tracker.lock().unwrap_or_else(|e| e.into_inner());
        let mut last_access = HashMap::new();
        let mut access_count = HashMap::new();

        for id in point_tiers.keys() {
            if let Some(ts) = tracker.last_access(id) {
                last_access.insert(id.clone(), ts);
            }
            let count = tracker.access_count(id);
            if count > 0 {
                access_count.insert(id.clone(), count);
            }
        }

        Self {
            point_tiers,
            last_access,
            access_count,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::CollectionConfig;
    use crate::quantization::QuantizationConfig;
    use crate::search::{DistanceMetric, HnswConfig};

    fn test_config() -> CollectionConfig {
        CollectionConfig {
            name: "test_tiered".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        }
    }

    fn tiered_config() -> TieredStorageConfig {
        TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 3600,
        }
    }

    fn make_point(id: &str, vector: Vec<f32>) -> Point {
        Point::new(id, vector, serde_json::Value::Null).unwrap()
    }

    // ─── AccessTracker tests ─────────────────────────────────────────

    #[test]
    fn test_access_tracker_record_and_get_tier() {
        let mut tracker = AccessTracker::new();
        let config = tiered_config();

        let now = unix_now();

        // Ponto acessado agora → Hot
        tracker.record_access_at("p1", now);
        assert_eq!(tracker.get_tier_at("p1", &config, now), StorageTier::Hot);

        // Ponto acessado há 25 horas → Warm
        tracker.record_access_at("p2", now - 25 * 3600);
        assert_eq!(tracker.get_tier_at("p2", &config, now), StorageTier::Warm);

        // Ponto acessado há 200 horas → Cold
        tracker.record_access_at("p3", now - 200 * 3600);
        assert_eq!(tracker.get_tier_at("p3", &config, now), StorageTier::Cold);

        // Ponto nunca acessado → Cold
        assert_eq!(tracker.get_tier_at("p4", &config, now), StorageTier::Cold);
    }

    #[test]
    fn test_access_tracker_access_count() {
        let mut tracker = AccessTracker::new();

        assert_eq!(tracker.access_count("p1"), 0);

        tracker.record_access("p1");
        assert_eq!(tracker.access_count("p1"), 1);

        tracker.record_access("p1");
        tracker.record_access("p1");
        assert_eq!(tracker.access_count("p1"), 3);
    }

    #[test]
    fn test_tier_demotion() {
        let mut tracker = AccessTracker::new();
        let config = tiered_config();

        let now = unix_now();

        // Simula: p1 acessado recentemente, p2 há 30 horas, p3 há 200 horas
        tracker.record_access_at("p1", now);
        tracker.record_access_at("p2", now - 30 * 3600);
        tracker.record_access_at("p3", now - 200 * 3600);

        // Todos começam como Hot
        let mut current_tiers = HashMap::new();
        current_tiers.insert("p1".to_string(), StorageTier::Hot);
        current_tiers.insert("p2".to_string(), StorageTier::Hot);
        current_tiers.insert("p3".to_string(), StorageTier::Hot);

        let changes = tracker.points_to_demote_at(&config, &current_tiers, now);

        // p1 deve ficar Hot, p2 deve ir para Warm, p3 deve ir para Cold
        assert_eq!(changes.len(), 2);
        let change_map: HashMap<_, _> = changes.into_iter().collect();
        assert_eq!(change_map.get("p2"), Some(&StorageTier::Warm));
        assert_eq!(change_map.get("p3"), Some(&StorageTier::Cold));
    }

    #[test]
    fn test_tier_promotion() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_config = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 3600,
        };

        let tmp = std::env::temp_dir().join("ferres_test_tiered_promote");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_config, Some(&tmp)).unwrap();

        // Insere um ponto
        tc.insert(make_point("p1", vec![1.0, 0.0, 0.0])).unwrap();
        assert_eq!(tc.point_tier("p1"), Some(StorageTier::Hot));

        // Demove para warm
        tc.demote_to_warm("p1").unwrap();
        assert_eq!(tc.point_tier("p1"), Some(StorageTier::Warm));

        // Acessa o ponto (deve poder ler do warm)
        let point = tc.get("p1").unwrap();
        assert!(point.is_some());
        assert_eq!(point.unwrap().vector, vec![1.0, 0.0, 0.0]);

        // Promove de volta para Hot
        tc.promote_to_hot("p1").unwrap();
        assert_eq!(tc.point_tier("p1"), Some(StorageTier::Hot));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cold_storage_roundtrip() {
        let tmp = std::env::temp_dir().join("ferres_test_cold_rt");
        let _ = fs::remove_dir_all(&tmp);

        let cold = ColdStorage::new(&tmp).unwrap();
        let point = make_point("cold1", vec![1.0, 2.0, 3.0]);

        cold.save_point(&point).unwrap();
        assert!(cold.contains("cold1"));

        let loaded = cold.load_point("cold1").unwrap();
        assert_eq!(loaded.id, "cold1");
        assert_eq!(loaded.vector, vec![1.0, 2.0, 3.0]);

        cold.remove_point("cold1");
        assert!(!cold.contains("cold1"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cold_point_path_no_collision() {
        let tmp = std::env::temp_dir().join("ferres_test_cold_no_collision");
        let _ = fs::remove_dir_all(&tmp);

        let cold = ColdStorage::new(&tmp).unwrap();

        // "doc/1" e "doc_1" devem gerar paths diferentes (antes collidiam)
        let path_slash = cold.point_path("doc/1");
        let path_underscore = cold.point_path("doc_1");

        assert_ne!(
            path_slash, path_underscore,
            "IDs 'doc/1' and 'doc_1' must map to different file paths"
        );

        // Verificação extra: os nomes devem ser os esperados
        assert!(path_slash.to_string_lossy().contains("doc%2F1.json"));
        assert!(path_underscore.to_string_lossy().contains("doc_1.json"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cold_point_path_special_chars() {
        let tmp = std::env::temp_dir().join("ferres_test_cold_special");
        let _ = fs::remove_dir_all(&tmp);

        let cold = ColdStorage::new(&tmp).unwrap();

        let ids = [
            "simple",
            "with/slash",
            "with:colon",
            "with space",
            "with/multiple/slashes",
            "unicode_café",
            "mix/of:special chars!",
        ];

        // Todos devem gerar paths distintas
        let paths: Vec<PathBuf> = ids.iter().map(|id| cold.point_path(id)).collect();
        for (i, p1) in paths.iter().enumerate() {
            for (j, p2) in paths.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        p1, p2,
                        "IDs '{}' and '{}' must map to different paths",
                        ids[i], ids[j]
                    );
                }
            }
        }

        // Verificar que os paths não contêm caracteres problemáticos de filesystem
        for (id, path) in ids.iter().zip(paths.iter()) {
            let filename = path.file_name().unwrap().to_string_lossy();
            assert!(
                !filename.contains('/') && !filename.contains('\\'),
                "path for ID '{}' contains filesystem separators: {}",
                id,
                filename
            );
            assert!(
                filename.ends_with(".json"),
                "path for ID '{}' must end with .json: {}",
                id,
                filename
            );
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cold_roundtrip_special_id() {
        let tmp = std::env::temp_dir().join("ferres_test_cold_special_rt");
        let _ = fs::remove_dir_all(&tmp);

        let cold = ColdStorage::new(&tmp).unwrap();

        let special_ids = ["doc/1", "ns:item:42", "hello world", "café☕"];

        for id in &special_ids {
            let point = make_point(id, vec![1.0, 2.0, 3.0]);
            cold.save_point(&point).unwrap();
            assert!(cold.contains(id), "cold storage should contain '{}'", id);

            let loaded = cold.load_point(id).unwrap();
            assert_eq!(loaded.id, *id);
            assert_eq!(loaded.vector, vec![1.0, 2.0, 3.0]);

            cold.remove_point(id);
            assert!(
                !cold.contains(id),
                "cold storage should not contain '{}' after removal",
                id
            );
        }

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_warm_storage_roundtrip() {
        let tmp = std::env::temp_dir().join("ferres_test_warm_rt");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut warm = WarmStorage::new(&tmp, 3).unwrap();

        warm.add_vector("w1", &[1.0, 2.0, 3.0]).unwrap();
        assert!(warm.contains("w1"));
        assert_eq!(warm.len(), 1);

        let vec = warm.read_vector("w1").unwrap();
        assert_eq!(vec, vec![1.0, 2.0, 3.0]);

        warm.remove_vector("w1");
        assert!(!warm.contains("w1"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_warm_storage_flush() {
        let tmp = std::env::temp_dir().join("ferres_test_warm_flush");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut warm = WarmStorage::new(&tmp, 3).unwrap();

        warm.add_vector("f1", &[1.0, 2.0, 3.0]).unwrap();
        warm.add_vector("f2", &[4.0, 5.0, 6.0]).unwrap();

        // Flush força tudo para disco
        warm.flush().unwrap();

        // Reabre e verifica que dados persistem
        let warm2 = WarmStorage::new(&tmp, 3).unwrap();
        assert_eq!(warm2.len(), 2);
        assert!(warm2.contains("f1"));
        assert!(warm2.contains("f2"));
        assert_eq!(warm2.read_vector("f1").unwrap(), vec![1.0, 2.0, 3.0]);
        assert_eq!(warm2.read_vector("f2").unwrap(), vec![4.0, 5.0, 6.0]);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_warm_storage_atomic_index() {
        let tmp = std::env::temp_dir().join("ferres_test_warm_atomic_idx");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut warm = WarmStorage::new(&tmp, 3).unwrap();
        warm.add_vector("a1", &[1.0, 2.0, 3.0]).unwrap();

        // Após add_vector, o arquivo .tmp deve ter sido renomeado (não deve existir)
        let tmp_index = tmp.join("warm_index.json.tmp");
        assert!(
            !tmp_index.exists(),
            "warm_index.json.tmp should not exist after successful save"
        );

        // O índice final deve existir
        let index_path = tmp.join("warm_index.json");
        assert!(index_path.exists(), "warm_index.json should exist");

        // Verifica conteúdo do índice
        let index_data: HashMap<String, usize> =
            serde_json::from_str(&fs::read_to_string(&index_path).unwrap()).unwrap();
        assert!(index_data.contains_key("a1"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_search_across_tiers() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_config = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 3600,
        };

        let tmp = std::env::temp_dir().join("ferres_test_tiered_search");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_config, Some(&tmp)).unwrap();

        // Insere 5 pontos (mais pontos = grafo HNSW mais bem conectado,
        // evitando flakiness com grafos muito pequenos).
        tc.insert(make_point("hot1", vec![1.0, 0.0, 0.0])).unwrap();
        tc.insert(make_point("hot2", vec![0.5, 0.5, 0.0])).unwrap();
        tc.insert(make_point("hot3", vec![0.0, 0.0, 1.0])).unwrap();
        tc.insert(make_point("warm1", vec![0.0, 1.0, 0.0])).unwrap();
        tc.insert(make_point("cold1", vec![0.9, 0.1, 0.0])).unwrap();

        // Demove warm1 e cold1
        tc.demote_to_warm("warm1").unwrap();
        tc.demote_to_warm("cold1").unwrap();
        tc.demote_to_cold("cold1").unwrap();

        assert_eq!(tc.point_tier("hot1"), Some(StorageTier::Hot));
        assert_eq!(tc.point_tier("hot2"), Some(StorageTier::Hot));
        assert_eq!(tc.point_tier("hot3"), Some(StorageTier::Hot));
        assert_eq!(tc.point_tier("warm1"), Some(StorageTier::Warm));
        assert_eq!(tc.point_tier("cold1"), Some(StorageTier::Cold));

        // Busca HNSW retorna IDs de pontos em TODOS os tiers
        // (porque demote usa remove_data_only, sem tombstone no HNSW).
        let results = tc.search(&[1.0, 0.0, 0.0], 5).unwrap();
        assert_eq!(
            results.len(),
            5,
            "HNSW search must return points from all tiers"
        );

        // hot1 deve ser o mais próximo de [1,0,0]
        assert_eq!(results[0].0, "hot1");

        // Pontos em todos os tiers devem estar presentes
        let result_ids: std::collections::HashSet<&str> =
            results.iter().map(|r| r.0.as_str()).collect();
        assert!(
            result_ids.contains("hot1"),
            "hot point must appear in results"
        );
        assert!(
            result_ids.contains("warm1"),
            "warm point must appear in results"
        );
        assert!(
            result_ids.contains("cold1"),
            "cold point must appear in results"
        );

        // get_from_any_tier deve encontrar pontos em todos os tiers
        let hot_point = tc.get_from_any_tier("hot1");
        assert!(
            hot_point.is_some(),
            "get_from_any_tier must find hot points"
        );
        assert_eq!(hot_point.unwrap().vector, vec![1.0, 0.0, 0.0]);

        let warm_point = tc.get_from_any_tier("warm1");
        assert!(
            warm_point.is_some(),
            "get_from_any_tier must find warm points"
        );
        assert_eq!(warm_point.unwrap().vector, vec![0.0, 1.0, 0.0]);

        let cold_point = tc.get_from_any_tier("cold1");
        assert!(
            cold_point.is_some(),
            "get_from_any_tier must find cold points"
        );
        assert_eq!(cold_point.unwrap().vector, vec![0.9, 0.1, 0.0]);

        // Ponto inexistente retorna None
        assert!(tc.get_from_any_tier("nonexistent").is_none());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_search_with_filter_across_tiers() {
        use serde_json::json;

        let config = CollectionConfig {
            name: "test_filter_tiered".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig {
                ef_search: 100, // garante exploração suficiente para 4 pontos
                ..HnswConfig::default()
            },
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        };
        let collection = Collection::new(config);
        let tiered_cfg = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 3600,
        };

        let tmp = std::env::temp_dir().join("ferres_test_tiered_filter_search");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_cfg, Some(&tmp)).unwrap();

        // Insere pontos com metadata variada
        let p_hot = Point::new(
            "hot_tech",
            vec![1.0, 0.0, 0.0],
            json!({"category": "tech", "status": "active"}),
        )
        .unwrap();
        let p_warm = Point::new(
            "warm_tech",
            vec![0.9, 0.1, 0.0],
            json!({"category": "tech", "status": "inactive"}),
        )
        .unwrap();
        let p_cold = Point::new(
            "cold_sci",
            vec![0.0, 1.0, 0.0],
            json!({"category": "science", "status": "active"}),
        )
        .unwrap();
        let p_warm2 = Point::new(
            "warm_tech2",
            vec![0.8, 0.2, 0.0],
            json!({"category": "tech", "status": "active"}),
        )
        .unwrap();

        tc.insert(p_hot).unwrap();
        tc.insert(p_warm).unwrap();
        tc.insert(p_cold).unwrap();
        tc.insert(p_warm2).unwrap();

        // Demove pontos para diferentes tiers
        tc.demote_to_warm("warm_tech").unwrap();
        tc.demote_to_warm("warm_tech2").unwrap();
        tc.demote_to_warm("cold_sci").unwrap();
        tc.demote_to_cold("cold_sci").unwrap();

        assert_eq!(tc.point_tier("hot_tech"), Some(StorageTier::Hot));
        assert_eq!(tc.point_tier("warm_tech"), Some(StorageTier::Warm));
        assert_eq!(tc.point_tier("warm_tech2"), Some(StorageTier::Warm));
        assert_eq!(tc.point_tier("cold_sci"), Some(StorageTier::Cold));

        // HNSW search deve retornar ao menos os pontos mais próximos.
        // cold_sci ([0,1,0]) é ortogonal à query ([1,0,0]); com grafos HNSW
        // probabilísticos de colações muito pequenas o nó distante pode ficar
        // fora do caminho de busca — toleramos >= 3.
        let all_results = tc.search(&[1.0, 0.0, 0.0], 10).unwrap();
        assert!(
            all_results.len() >= 3,
            "HNSW must return at least 3 points (got {})",
            all_results.len()
        );

        // Verifica filtro cross-tier via get_from_any_tier sobre IDs conhecidos.
        // Não depende do HNSW retornar todos os pontos (aproximado); testa
        // apenas que a recuperação de metadados funciona em cada tier.
        let all_ids = ["hot_tech", "warm_tech", "warm_tech2", "cold_sci"];

        // Filtro: category=tech  →  hot_tech (Hot), warm_tech (Warm), warm_tech2 (Warm)
        let tech_ids: std::collections::HashSet<&str> = all_ids
            .iter()
            .filter(|&&id| {
                tc.get_from_any_tier(id)
                    .map(|p| p.metadata.get("category") == Some(&json!("tech")))
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        assert_eq!(
            tech_ids.len(),
            3,
            "filter category=tech must match 3 points, got: {:?}",
            tech_ids
        );
        assert!(
            tech_ids.contains("hot_tech"),
            "hot_tech must pass category=tech filter"
        );
        assert!(
            tech_ids.contains("warm_tech"),
            "warm_tech must pass category=tech filter"
        );
        assert!(
            tech_ids.contains("warm_tech2"),
            "warm_tech2 must pass category=tech filter"
        );

        // Filtro: category=tech AND status=active  →  hot_tech (Hot), warm_tech2 (Warm)
        let active_tech_ids: std::collections::HashSet<&str> = all_ids
            .iter()
            .filter(|&&id| {
                tc.get_from_any_tier(id)
                    .map(|p| {
                        p.metadata.get("category") == Some(&json!("tech"))
                            && p.metadata.get("status") == Some(&json!("active"))
                    })
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        assert_eq!(
            active_tech_ids.len(),
            2,
            "filter category=tech AND status=active must match 2 points, got: {:?}",
            active_tech_ids
        );
        assert!(active_tech_ids.contains("hot_tech"));
        assert!(active_tech_ids.contains("warm_tech2"));

        // Filtro: category=science  →  apenas cold_sci (Cold)
        let sci_ids: Vec<&str> = all_ids
            .iter()
            .filter(|&&id| {
                tc.get_from_any_tier(id)
                    .map(|p| p.metadata.get("category") == Some(&json!("science")))
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        assert_eq!(
            sci_ids.len(),
            1,
            "only cold_sci should match category=science"
        );
        assert_eq!(sci_ids[0], "cold_sci");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tier_distribution() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_config = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 3600,
        };

        let tmp = std::env::temp_dir().join("ferres_test_tier_dist");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_config, Some(&tmp)).unwrap();

        tc.insert(make_point("p1", vec![1.0, 0.0, 0.0])).unwrap();
        tc.insert(make_point("p2", vec![0.0, 1.0, 0.0])).unwrap();
        tc.insert(make_point("p3", vec![0.0, 0.0, 1.0])).unwrap();

        let dist = tc.tier_distribution();
        assert_eq!(dist.hot, 3);
        assert_eq!(dist.warm, 0);
        assert_eq!(dist.cold, 0);

        // Demove p2 para warm
        tc.demote_to_warm("p2").unwrap();
        let dist = tc.tier_distribution();
        assert_eq!(dist.hot, 2);
        assert_eq!(dist.warm, 1);
        assert_eq!(dist.cold, 0);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_compaction() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_cfg = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 1,
            warm_threshold_hours: 5,
            compaction_interval_secs: 1,
        };

        let tmp = std::env::temp_dir().join("ferres_test_compaction");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_cfg, Some(&tmp)).unwrap();

        // Insere pontos
        tc.insert(make_point("recent", vec![1.0, 0.0, 0.0]))
            .unwrap();
        tc.insert(make_point("old", vec![0.0, 1.0, 0.0])).unwrap();

        // Simula que "old" foi acessado há 2 horas (> hot_threshold=1, < warm_threshold=5)
        let now = unix_now();
        if let Ok(mut tracker) = tc.access_tracker.lock() {
            tracker.record_access_at("old", now - 2 * 3600);
        }

        // Roda compactação
        let result = tc.run_compaction().unwrap();
        assert!(result.total_changes > 0);

        // "old" deve ter sido demovido para Warm (2h > hot_threshold=1h, < warm_threshold=5h)
        assert_eq!(tc.point_tier("old"), Some(StorageTier::Warm));
        assert_eq!(tc.point_tier("recent"), Some(StorageTier::Hot));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_compaction_large_collection() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_cfg = TieredStorageConfig {
            enabled: true,
            hot_threshold_hours: 24,
            warm_threshold_hours: 168,
            compaction_interval_secs: 1,
        };

        let tmp = std::env::temp_dir().join("ferres_test_compaction_large");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_cfg, Some(&tmp)).unwrap();

        // Insere 1000 pontos
        for i in 0..1000 {
            let mut vec = vec![0.0f32; 3];
            vec[i % 3] = 1.0;
            tc.insert(make_point(&format!("p{i}"), vec)).unwrap();
        }

        let now = unix_now();

        // Simula acessos variados: 0..300 recentes (Hot), 300..700 há 2 dias (Warm), 700..1000 há 10 dias (Cold)
        if let Ok(mut tracker) = tc.access_tracker.lock() {
            for i in 0..300 {
                tracker.record_access_at(&format!("p{i}"), now);
            }
            for i in 300..700 {
                tracker.record_access_at(&format!("p{i}"), now - 48 * 3600);
            }
            for i in 700..1000 {
                tracker.record_access_at(&format!("p{i}"), now - 240 * 3600);
            }
        }

        // Roda compactação
        let result = tc.run_compaction().unwrap();

        // Verifica distribuição: ~300 hot, ~400 warm, ~300 cold
        let dist = tc.tier_distribution();
        assert_eq!(dist.hot, 300, "expected 300 hot");
        assert_eq!(dist.warm, 400, "expected 400 warm");
        assert_eq!(dist.cold, 300, "expected 300 cold");

        assert_eq!(
            result.demoted_to_warm, 700,
            "400 hot->warm + 300 hot->cold (then cold batch)"
        );
        assert_eq!(result.demoted_to_cold, 300, "300 demoted to cold");

        // Amostra: p0 deve estar Hot, p400 Warm, p700 Cold
        assert_eq!(tc.point_tier("p0"), Some(StorageTier::Hot));
        assert_eq!(tc.point_tier("p400"), Some(StorageTier::Warm));
        assert_eq!(tc.point_tier("p700"), Some(StorageTier::Cold));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_tiered_disabled_everything_hot() {
        let config = test_config();
        let collection = Collection::new(config);
        let disabled_config = TieredStorageConfig::default(); // enabled: false

        let mut tc = TieredCollection::new(collection, disabled_config, None).unwrap();

        tc.insert(make_point("p1", vec![1.0, 0.0, 0.0])).unwrap();
        assert_eq!(tc.point_tier("p1"), Some(StorageTier::Hot));

        // Compactação não faz nada quando desabilitado
        let result = tc.run_compaction().unwrap();
        assert_eq!(result.total_changes, 0);
    }

    #[test]
    fn test_tier_metadata_serialization() {
        let config = test_config();
        let collection = Collection::new(config);
        let tiered_cfg = tiered_config();

        let tmp = std::env::temp_dir().join("ferres_test_tier_meta_ser");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut tc = TieredCollection::new(collection, tiered_cfg, Some(&tmp)).unwrap();
        tc.insert(make_point("p1", vec![1.0, 0.0, 0.0])).unwrap();

        let meta = TierMetadata::from_tiered_collection(&tc);
        assert_eq!(meta.point_tiers.len(), 1);
        assert_eq!(meta.point_tiers.get("p1"), Some(&StorageTier::Hot));
        assert!(meta.last_access.contains_key("p1"));
        assert_eq!(meta.access_count.get("p1"), Some(&1));

        // Serialização JSON roundtrip
        let json = serde_json::to_string(&meta).unwrap();
        let restored: TierMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.point_tiers.len(), 1);

        let _ = fs::remove_dir_all(&tmp);
    }

    // ─── Benchmark de latência ─────────────────────────────────────

    #[test]
    fn bench_search_latency_hot_vs_cold() {
        use std::time::Instant;

        let config = CollectionConfig {
            name: "bench_tiered".to_string(),
            dimension: 128,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        };

        let mut collection = Collection::new(config);

        // Insere 100 pontos
        for i in 0..100 {
            let mut vector = vec![0.0f32; 128];
            vector[i % 128] = 1.0;
            collection
                .insert(make_point(&format!("p{i}"), vector))
                .unwrap();
        }

        // Mede latência com todos os pontos hot
        let query = vec![1.0f32; 128];
        let start = Instant::now();
        for _ in 0..100 {
            let _ = collection.search(&query, 10, None, None);
        }
        let hot_duration = start.elapsed();

        // O benchmark é informativo — apenas verifica que não panics
        debug!(
            hot_ms = hot_duration.as_millis(),
            "search latency benchmark"
        );
    }
}
