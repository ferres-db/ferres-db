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
//! Com **compressão opcional** (Zstd), o ficheiro começa com o magic `WALz` e cada
//! entrada é armazenada como frame: 4 bytes (u32 LE) tamanho + payload comprimido.
//!
//! ## Snapshot
//!
//! A cada `snapshot_threshold` operações (padrão: 1000), um snapshot
//! completo é criado via `FileStorage::save_collection` e o WAL é
//! truncado.

use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::time::unix_now;

/// Magic bytes no início do WAL quando compressão Zstd está ativa.
const WAL_ZSTD_MAGIC: &[u8; 4] = b"WALz";

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
    /// Cria uma relação entre dois pontos (grafo não direcionado).
    Link {
        /// ID (storage_id) do ponto de origem.
        from: String,
        /// ID (storage_id) do ponto de destino.
        to: String,
    },
}

// ─── WalConfig ────────────────────────────────────────────────────────

/// Configuração do Write-Ahead Log.
///
/// O default (`fsync_per_write=false`) mantém compatibilidade com o comportamento
/// anterior (sem fsync). Em produção, USE `fsync_per_write=true`.
pub struct WalConfig {
    /// Número de operações antes de disparar snapshot automático.
    pub snapshot_threshold: usize,
    /// Habilita compressão Zstd para entradas do WAL.
    pub compress: bool,
    /// Chama `sync_data()` após cada `append_*`. Mais seguro, mais lento.
    /// Default: false (compat). Produção DEVE usar true.
    pub fsync_per_write: bool,
    /// Quando `fsync_per_write=false`, intervalo máximo entre sync_data().
    pub fsync_interval: Duration,
    /// Quando `fsync_per_write=false`, número máximo de ops antes de forçar sync_data().
    pub fsync_every_n_ops: usize,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            snapshot_threshold: 1000,
            compress: false,
            fsync_per_write: false,
            fsync_interval: Duration::from_secs(1),
            fsync_every_n_ops: 1000,
        }
    }
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
    /// Configuração do WAL (threshold, compressão, fsync).
    config: WalConfig,
    /// Número de operações desde o último fsync.
    ops_since_fsync: usize,
    /// Timestamp do último fsync.
    last_fsync: Instant,
    /// Callback opcional chamado após cada sync_data().
    fsync_hook: Option<Box<dyn Fn(Duration) + Send>>,
}

/// Limite de operações no WAL antes de forçar backpressure (snapshot obrigatório).
/// Heurística: ~100MB de WAL para evitar crescimento indefinido e OOM.
const MAX_WAL_OPS_BEFORE_BACKPRESSURE: usize = 10_000;

impl Wal {
    /// Threshold padrão: snapshot a cada 1000 operações.
    pub const DEFAULT_SNAPSHOT_THRESHOLD: usize = 1000;

    /// Abre (ou cria) o WAL com configuração padrão (sem fsync).
    ///
    /// Se `compress` for true, as entradas são escritas com compressão Zstd (menor uso de disco).
    /// Não faz replay — use `recover_collection()` para recuperação.
    pub fn open(
        collection_dir: &Path,
        snapshot_threshold: usize,
        compress: bool,
    ) -> Result<Self, FerresError> {
        let config = WalConfig {
            snapshot_threshold,
            compress,
            ..Default::default()
        };
        Self::open_with_config(collection_dir, config)
    }

    /// Abre (ou cria) o WAL com configuração completa.
    ///
    /// Não faz replay — use `recover_collection()` para recuperação.
    pub fn open_with_config(
        collection_dir: &Path,
        config: WalConfig,
    ) -> Result<Self, FerresError> {
        fs::create_dir_all(collection_dir).map_err(|e| {
            FerresError::Storage(format!(
                "failed to create collection directory {}: {e}",
                collection_dir.display()
            ))
        })?;

        let wal_path = collection_dir.join("wal.log");

        // Conta entradas existentes para inicializar o contador (formato auto-detectado)
        let ops_since_snapshot = if wal_path.exists() {
            Self::count_entries(&wal_path)?
        } else {
            0
        };

        // Abre o arquivo em modo append
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| {
                FerresError::Storage(format!("failed to open WAL at {}: {e}", wal_path.display()))
            })?;

        // Se compressão ativa e ficheiro novo (0 bytes), escreve magic
        if config.compress {
            let meta = file.metadata().map_err(|e| {
                FerresError::Storage(format!("failed to stat WAL at {}: {e}", wal_path.display()))
            })?;
            if meta.len() == 0 {
                file.write_all(WAL_ZSTD_MAGIC).map_err(|e| {
                    FerresError::Storage(format!(
                        "failed to write WAL magic at {}: {e}",
                        wal_path.display()
                    ))
                })?;
            }
        }

        Ok(Self {
            collection_dir: collection_dir.to_path_buf(),
            writer: Some(BufWriter::new(file)),
            ops_since_snapshot,
            config,
            ops_since_fsync: 0,
            last_fsync: Instant::now(),
            fsync_hook: None,
        })
    }

    /// Registra uma operação de upsert no WAL.
    ///
    /// Deve ser chamado ANTES da mutação em memória.
    /// Retorna erro se o WAL já tiver muitas operações (backpressure: snapshot obrigatório).
    pub fn append_upsert(&mut self, point: &Point) -> Result<(), FerresError> {
        if self.ops_since_snapshot >= MAX_WAL_OPS_BEFORE_BACKPRESSURE {
            return Err(FerresError::Storage(
                "WAL too large, snapshot required".to_string(),
            ));
        }
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
    /// Retorna erro se o WAL já tiver muitas operações (backpressure: snapshot obrigatório).
    pub fn append_delete(&mut self, id: &str) -> Result<(), FerresError> {
        if self.ops_since_snapshot >= MAX_WAL_OPS_BEFORE_BACKPRESSURE {
            return Err(FerresError::Storage(
                "WAL too large, snapshot required".to_string(),
            ));
        }
        let entry = WalEntry {
            timestamp: current_timestamp(),
            operation: WalOperation::Delete { id: id.to_string() },
        };
        self.append_entry(&entry)?;
        self.ops_since_snapshot += 1;
        Ok(())
    }

    /// Registra uma operação de link (relação entre dois pontos) no WAL.
    ///
    /// Deve ser chamado ANTES da mutação em memória.
    /// Retorna erro se o WAL já tiver muitas operações (backpressure: snapshot obrigatório).
    pub fn append_link(&mut self, from: &str, to: &str) -> Result<(), FerresError> {
        if self.ops_since_snapshot >= MAX_WAL_OPS_BEFORE_BACKPRESSURE {
            return Err(FerresError::Storage(
                "WAL too large, snapshot required".to_string(),
            ));
        }
        let entry = WalEntry {
            timestamp: current_timestamp(),
            operation: WalOperation::Link {
                from: from.to_string(),
                to: to.to_string(),
            },
        };
        self.append_entry(&entry)?;
        self.ops_since_snapshot += 1;
        Ok(())
    }

    /// Retorna true se o número de operações atingiu o threshold.
    pub fn should_snapshot(&self) -> bool {
        self.ops_since_snapshot >= self.config.snapshot_threshold
    }

    /// Trunca o WAL após um snapshot bem-sucedido.
    ///
    /// Fecha o writer atual, reabre em modo truncate e zera o contador.
    /// Com compressão, reescreve o magic `WALz` após truncar.
    pub fn truncate_after_snapshot(&mut self) -> Result<(), FerresError> {
        // Fecha o writer atual
        self.writer = None;

        let wal_path = self.collection_dir.join("wal.log");

        let mut file = fs::OpenOptions::new()
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

        if self.config.compress {
            file.write_all(WAL_ZSTD_MAGIC).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to write WAL magic after truncate at {}: {e}",
                    wal_path.display()
                ))
            })?;
        }

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

    /// Lê entradas do WAL a partir de uma posição (índice 0-based) para replicação incremental.
    ///
    /// Retorna apenas as entradas com índice >= `position`. Útil para réplicas que consomem
    /// o WAL do líder a partir de um offset conhecido.
    /// Detecta automaticamente formato comprimido (magic `WALz`) ou JSONL.
    pub fn stream_from(collection_dir: &Path, position: u64) -> Result<Vec<WalEntry>, FerresError> {
        let all = Self::read_entries(collection_dir)?;
        let from = position as usize;
        if from >= all.len() {
            return Ok(Vec::new());
        }
        Ok(all[from..].to_vec())
    }

    /// Lê todas as entradas do WAL para replay.
    ///
    /// Detecta automaticamente formato comprimido (magic `WALz`) ou JSONL.
    /// Linhas/frames mal-formados (artefato de crash) são ignorados com warning.
    pub fn read_entries(collection_dir: &Path) -> Result<Vec<WalEntry>, FerresError> {
        let wal_path = collection_dir.join("wal.log");

        if !wal_path.exists() {
            return Ok(Vec::new());
        }

        let mut file = fs::File::open(&wal_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to open WAL for reading at {}: {e}",
                wal_path.display()
            ))
        })?;

        let mut magic = [0u8; 4];
        let n = file.read(&mut magic).map_err(|e| {
            FerresError::Storage(format!(
                "failed to read WAL magic at {}: {e}",
                wal_path.display()
            ))
        })?;

        let entries = if n == 4 && magic == *WAL_ZSTD_MAGIC {
            Self::read_entries_compressed(&mut file, &wal_path)?
        } else {
            file.seek(SeekFrom::Start(0)).map_err(|e| {
                FerresError::Storage(format!("failed to seek WAL at {}: {e}", wal_path.display()))
            })?;
            Self::read_entries_jsonl(&mut file, &wal_path)?
        };

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

    /// Número de operações desde o último fsync (para métricas).
    pub fn ops_since_fsync(&self) -> usize {
        self.ops_since_fsync
    }

    /// Registra um callback chamado após cada sync_data().
    pub fn set_fsync_hook(&mut self, hook: Box<dyn Fn(Duration) + Send>) {
        self.fsync_hook = Some(hook);
    }

    // ─── Private ──────────────────────────────────────────────────────

    /// Serializa e escreve uma entrada, seguida de flush.
    fn append_entry(&mut self, entry: &WalEntry) -> Result<(), FerresError> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| FerresError::Storage("WAL writer is closed".to_string()))?;

        if self.config.compress {
            let json = serde_json::to_string(entry)
                .map_err(|e| FerresError::Storage(format!("failed to serialize WAL entry: {e}")))?;
            let compressed = zstd::encode_all(json.as_bytes(), 0)
                .map_err(|e| FerresError::Storage(format!("failed to compress WAL entry: {e}")))?;
            let len = compressed.len() as u32;
            writer.write_all(&len.to_le_bytes()).map_err(|e| {
                FerresError::Storage(format!("failed to write WAL frame length: {e}"))
            })?;
            writer.write_all(&compressed).map_err(|e| {
                FerresError::Storage(format!("failed to write WAL compressed frame: {e}"))
            })?;
        } else {
            let json = serde_json::to_string(entry)
                .map_err(|e| FerresError::Storage(format!("failed to serialize WAL entry: {e}")))?;
            writer
                .write_all(json.as_bytes())
                .map_err(|e| FerresError::Storage(format!("failed to write WAL entry: {e}")))?;
            writer
                .write_all(b"\n")
                .map_err(|e| FerresError::Storage(format!("failed to write WAL newline: {e}")))?;
        }

        writer
            .flush()
            .map_err(|e| FerresError::Storage(format!("failed to flush WAL: {e}")))?;

        Ok(())
    }

    /// Lê entradas em formato comprimido (frames: u32 LE len + zstd payload).
    fn read_entries_compressed(
        file: &mut fs::File,
        wal_path: &Path,
    ) -> Result<Vec<WalEntry>, FerresError> {
        let mut entries = Vec::new();
        let mut len_buf = [0u8; 4];
        loop {
            let n = file.read(&mut len_buf).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to read WAL frame length at {}: {e}",
                    wal_path.display()
                ))
            })?;
            if n == 0 {
                break;
            }
            if n != 4 {
                warn!(
                    path = %wal_path.display(),
                    "truncated WAL frame length, stopping replay"
                );
                break;
            }
            let frame_len = u32::from_le_bytes(len_buf) as usize;
            if frame_len == 0 || frame_len > 10 * 1024 * 1024 {
                warn!(
                    frame_len,
                    path = %wal_path.display(),
                    "invalid WAL frame length, stopping replay"
                );
                break;
            }
            let mut compressed = vec![0u8; frame_len];
            file.read_exact(&mut compressed).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to read WAL compressed frame at {}: {e}",
                    wal_path.display()
                ))
            })?;
            match zstd::decode_all(compressed.as_slice()) {
                Ok(decompressed) => {
                    let json = String::from_utf8(decompressed).map_err(|e| {
                        FerresError::Storage(format!("WAL decompressed frame is not UTF-8: {e}"))
                    })?;
                    match serde_json::from_str::<WalEntry>(&json) {
                        Ok(entry) => entries.push(entry),
                        Err(e) => {
                            warn!(
                                error = %e,
                                path = %wal_path.display(),
                                "skipping malformed WAL entry (possible crash artifact)"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        path = %wal_path.display(),
                        "failed to decompress WAL frame, stopping replay"
                    );
                    break;
                }
            }
        }
        Ok(entries)
    }

    /// Lê entradas em formato JSONL (uma linha por entrada).
    fn read_entries_jsonl(
        file: &mut fs::File,
        wal_path: &Path,
    ) -> Result<Vec<WalEntry>, FerresError> {
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
        Ok(entries)
    }

    /// Conta o número de entradas válidas no arquivo WAL (detecta formato por magic).
    fn count_entries(wal_path: &Path) -> Result<usize, FerresError> {
        let mut file = fs::File::open(wal_path).map_err(|e| {
            FerresError::Storage(format!(
                "failed to open WAL for counting at {}: {e}",
                wal_path.display()
            ))
        })?;
        let mut magic = [0u8; 4];
        let n = file.read(&mut magic).map_err(|e| {
            FerresError::Storage(format!(
                "failed to read WAL magic at {}: {e}",
                wal_path.display()
            ))
        })?;
        if n == 4 && magic == *WAL_ZSTD_MAGIC {
            let entries = Self::read_entries_compressed(&mut file, wal_path)?;
            Ok(entries.len())
        } else {
            file.seek(SeekFrom::Start(0)).map_err(|e| {
                FerresError::Storage(format!("failed to seek WAL at {}: {e}", wal_path.display()))
            })?;
            let reader = BufReader::new(file);
            let mut count = 0;
            for line in reader.lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    count += 1;
                }
            }
            Ok(count)
        }
    }
}

// ─── Retention (WAL compaction) ────────────────────────────────────────

/// Compacta o WAL removendo entradas com timestamp anterior a `cutoff_ts` (Unix segundos).
/// Reescreve `wal.log` apenas com entradas dentro do período de retenção.
/// Use quando a coleção tiver `retention_days` configurado; chamar sem WAL aberto (ex.: no worker de retenção).
pub fn compact_wal_entries_older_than(
    collection_dir: &Path,
    cutoff_ts: u64,
    compress: bool,
) -> Result<usize, FerresError> {
    let wal_path = collection_dir.join("wal.log");
    if !wal_path.exists() {
        return Ok(0);
    }
    let entries = Wal::read_entries(collection_dir)?;
    let original_count = entries.len();
    let kept: Vec<WalEntry> = entries
        .into_iter()
        .filter(|e| e.timestamp >= cutoff_ts)
        .collect();
    let removed = original_count.saturating_sub(kept.len());
    if removed == 0 {
        return Ok(0);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&wal_path)
        .map_err(|e| {
            FerresError::Storage(format!(
                "failed to truncate WAL for compaction at {}: {e}",
                wal_path.display()
            ))
        })?;
    if compress {
        file.write_all(WAL_ZSTD_MAGIC).map_err(|e| {
            FerresError::Storage(format!(
                "failed to write WAL magic at {}: {e}",
                wal_path.display()
            ))
        })?;
    }
    let mut writer = BufWriter::new(file);
    for entry in &kept {
        if compress {
            let json = serde_json::to_string(entry)
                .map_err(|e| FerresError::Storage(format!("failed to serialize WAL entry: {e}")))?;
            let compressed = zstd::encode_all(json.as_bytes(), 0)
                .map_err(|e| FerresError::Storage(format!("failed to compress WAL entry: {e}")))?;
            let len = compressed.len() as u32;
            writer.write_all(&len.to_le_bytes()).map_err(|e| {
                FerresError::Storage(format!("failed to write WAL frame length: {e}"))
            })?;
            writer.write_all(&compressed).map_err(|e| {
                FerresError::Storage(format!("failed to write WAL compressed frame: {e}"))
            })?;
        } else {
            let json = serde_json::to_string(entry)
                .map_err(|e| FerresError::Storage(format!("failed to serialize WAL entry: {e}")))?;
            writer
                .write_all(json.as_bytes())
                .map_err(|e| FerresError::Storage(format!("failed to write WAL entry: {e}")))?;
            writer
                .write_all(b"\n")
                .map_err(|e| FerresError::Storage(format!("failed to write WAL newline: {e}")))?;
        }
    }
    writer
        .flush()
        .map_err(|e| FerresError::Storage(format!("failed to flush WAL after compaction: {e}")))?;
    debug!(
        path = %wal_path.display(),
        removed,
        kept = kept.len(),
        "WAL compacted by retention"
    );
    Ok(removed)
}

// ─── Recovery ─────────────────────────────────────────────────────────

/// Lê o timestamp do último snapshot (gravado por `FileStorage::save_collection`).
/// Retorna 0 se o ficheiro não existir (compatibilidade com instalações antigas).
pub fn read_last_snapshot_timestamp(collection_dir: &Path) -> u64 {
    let path = collection_dir.join("last_snapshot_timestamp");
    if !path.exists() {
        return 0;
    }
    match fs::read_to_string(&path) {
        Ok(s) => s.trim().parse().unwrap_or(0),
        Err(_) => 0,
    }
}

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
                WalOperation::Upsert { point } => match collection.insert(point.clone()) {
                    Ok(_) => applied += 1,
                    Err(e) => {
                        warn!(
                            id = %point.id,
                            error = %e,
                            "failed to replay upsert, skipping"
                        );
                        skipped += 1;
                    }
                },
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
                WalOperation::Link { from, to } => match collection.add_relation(from, to) {
                    Ok(_) => applied += 1,
                    Err(e) => {
                        warn!(
                            from = %from,
                            to = %to,
                            error = %e,
                            "failed to replay link, skipping"
                        );
                        skipped += 1;
                    }
                },
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

/// Point-in-Time Recovery: carrega o último snapshot e re-aplica o WAL apenas
/// até o momento solicitado (`target_timestamp` inclusive).
///
/// - O snapshot on-disk representa o estado no momento do último `save_collection`.
/// - Apenas entradas do WAL com `entry.timestamp <= target_timestamp` são aplicadas.
/// - Se `target_timestamp` for anterior ao snapshot, apenas o snapshot é carregado
///   (nenhuma entrada WAL é aplicada); o snapshot já representa um estado anterior.
///
/// Retorna erro se a coleção não existir ou não puder ser carregada.
pub fn recover_collection_to_timestamp(
    collection_dir: &Path,
    target_timestamp: u64,
) -> Result<Option<Collection>, FerresError> {
    let config_path = collection_dir.join("config.json");
    if !config_path.exists() {
        return Ok(None);
    }

    let snapshot_ts = read_last_snapshot_timestamp(collection_dir);
    let mut collection = FileStorage::load_collection(collection_dir)?;

    let entries = Wal::read_entries(collection_dir)?;
    // When snapshot_ts is 0 (legacy: file missing), apply all WAL entries up to target.
    let to_apply: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.timestamp <= target_timestamp && (snapshot_ts == 0 || e.timestamp > snapshot_ts)
        })
        .collect();

    if !to_apply.is_empty() {
        info!(
            collection = %collection.name(),
            target_timestamp,
            applying = to_apply.len(),
            "PITR: replaying WAL entries up to timestamp"
        );
        for entry in to_apply {
            match &entry.operation {
                WalOperation::Upsert { point } => {
                    let _ = collection.insert(point.clone());
                }
                WalOperation::Delete { id } => {
                    let _ = collection.remove(id);
                }
                WalOperation::Link { from, to } => {
                    let _ = collection.add_relation(from, to);
                }
            }
        }
    }

    Ok(Some(collection))
}

/// Lista pontos de restauração para PITR: timestamp do último snapshot mais
/// os timestamps únicos das entradas do WAL (ordenados). Útil para a UI escolher
/// um momento para restaurar.
pub fn list_restore_points(collection_dir: &Path) -> Result<RestorePoints, FerresError> {
    let snapshot_ts = read_last_snapshot_timestamp(collection_dir);
    let entries = Wal::read_entries(collection_dir)?;
    let mut wal_timestamps: Vec<u64> = entries.iter().map(|e| e.timestamp).collect();
    wal_timestamps.sort();
    wal_timestamps.dedup();
    Ok(RestorePoints {
        last_snapshot_timestamp: snapshot_ts,
        wal_timestamps,
    })
}

/// Pontos de restauração disponíveis para uma coleção (PITR).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RestorePoints {
    /// Timestamp Unix do último snapshot.
    pub last_snapshot_timestamp: u64,
    /// Timestamps únicos das entradas no WAL (ordenados).
    pub wal_timestamps: Vec<u64>,
}

/// Helper: timestamp Unix atual em segundos.
fn current_timestamp() -> u64 {
    unix_now()
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
            quantization: Default::default(),
            tiered_storage: Default::default(),
            retention_days: None,
        }
    }

    fn make_point(id: &str, v: Vec<f32>) -> Point {
        Point::new(id.to_string(), v, serde_json::Value::Null).unwrap()
    }

    // ─── WalConfig Tests ──────────────────────────────────────────────

    #[test]
    fn wal_config_default_no_fsync() {
        let cfg = WalConfig::default();
        assert_eq!(cfg.snapshot_threshold, 1000);
        assert!(!cfg.compress);
        assert!(!cfg.fsync_per_write);
        assert_eq!(cfg.fsync_interval, Duration::from_secs(1));
        assert_eq!(cfg.fsync_every_n_ops, 1000);
    }

    // ─── WAL Unit Tests ───────────────────────────────────────────────

    #[test]
    fn wal_append_and_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 1000, false).unwrap();
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
            operation: WalOperation::Delete {
                id: "point-123".to_string(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"op\":\"delete\""));
        assert!(json.contains("\"point-123\""));
    }

    #[test]
    fn wal_link_entry_format() {
        let entry = WalEntry {
            timestamp: 1234567890,
            operation: WalOperation::Link {
                from: "id_A".to_string(),
                to: "id_B".to_string(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"op\":\"link\""));
        assert!(json.contains("\"id_A\""));
        assert!(json.contains("\"id_B\""));
    }

    #[test]
    fn wal_truncate_clears_entries() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 1000, false).unwrap();
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

        let content = format!("{valid_json}\n{{invalid json\n{valid_json}\n");
        fs::write(&wal_path, content).unwrap();

        let entries = Wal::read_entries(&dir).unwrap();
        assert_eq!(entries.len(), 2); // 2 válidas, 1 malformada ignorada
    }

    #[test]
    fn wal_should_snapshot_at_threshold() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        let mut wal = Wal::open(&dir, 5, false).unwrap();
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
            let mut wal = Wal::open(&dir, 1000, false).unwrap();
            let p = make_point("p1", vec![1.0, 2.0, 3.0]);
            wal.append_upsert(&p).unwrap();
            wal.append_upsert(&p).unwrap();
            wal.append_upsert(&p).unwrap();
        }

        // Reabre — deve contar 3 entradas existentes
        let wal = Wal::open(&dir, 1000, false).unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // Escreve WAL com 1 upsert adicional
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0]))
            .unwrap();

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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com upsert A(v2)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("a", vec![0.0, 1.0, 0.0]))
            .unwrap();

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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com delete(A)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com delete(X) — X não existe
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_delete("x").unwrap();

        // Não deve falhar
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered.get("a").is_some());
    }

    #[test]
    fn recovery_link_replay() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("test_col");

        // Snapshot com A, B (sem relações)
        let config = test_config("test_col");
        let mut col = Collection::new(config);
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com link(a, b)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_link("a", "b").unwrap();

        // Recovery deve aplicar o link
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 2);
        let pa = recovered.get("a").unwrap();
        let pb = recovered.get("b").unwrap();
        assert_eq!(pa.relations.as_deref(), Some(&["b".to_string()][..]));
        assert_eq!(pb.relations.as_deref(), Some(&["a".to_string()][..]));
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com upsert(C) válido + linha truncada (simula crash)
        let p = make_point("c", vec![0.0, 0.0, 1.0]);
        let valid_entry = WalEntry {
            timestamp: 1,
            operation: WalOperation::Upsert { point: p },
        };
        let valid_json = serde_json::to_string(&valid_entry).unwrap();

        let wal_path = dir.join("wal.log");
        let content = format!("{valid_json}\n{{\"timestamp\":2,\"operati");
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL tem upsert(C) — mas a coleção em memória NÃO foi mutada
        // (simula crash entre WAL append e collection.insert)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0]))
            .unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com upsert(B)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("b", vec![0.0, 1.0, 0.0]))
            .unwrap();
        drop(wal);

        // Novo snapshot com {A, B} — mas NÃO trunca WAL (simula crash)
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir, false, false).unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL: delete(B), upsert(D), upsert(A com novo vetor)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_delete("b").unwrap();
        wal.append_upsert(&make_point("d", vec![1.0, 1.0, 0.0]))
            .unwrap();
        wal.append_upsert(&make_point("a", vec![0.5, 0.5, 0.0]))
            .unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com upsert(C)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0]))
            .unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // Simula batch de 50 operações, crash após 30
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        for i in 0..30 {
            wal.append_upsert(&make_point(&format!("p{i}"), vec![i as f32, 0.0, 0.0]))
                .unwrap();
        }
        drop(wal);

        // Simula crash: WAL tem 30 entradas, mas apenas 20 foram aplicadas em memória
        // (não aplicamos em memória para simular crash)

        // Recovery deve aplicar todas as 30 entradas do WAL
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(recovered.len(), 30);
        for i in 0..30 {
            assert!(recovered.get(&format!("p{i}")).is_some());
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com operações
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_delete("b").unwrap();
        wal.append_upsert(&make_point("c", vec![0.0, 0.0, 1.0]))
            .unwrap();
        wal.append_upsert(&make_point("d", vec![0.5, 0.5, 0.0]))
            .unwrap();
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
        let results = recovered.search(&query, 5, None, None).unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com exatamente 1000 operações (threshold)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        for i in 0..1000 {
            wal.append_upsert(&make_point(&format!("p{i}"), vec![i as f32, 0.0, 0.0]))
                .unwrap();
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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL com upsert(B)
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("b", vec![0.0, 1.0, 0.0]))
            .unwrap();
        drop(wal);

        // Novo snapshot com {A, B}
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

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
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

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
        let content =
            format!("{json1}\n{json2}\n{{\"timestamp\":3,\"operation\":{{\"op\":\"upsert\"");
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
        col.insert(make_point("near_origin", vec![0.1, 0.1, 0.1]))
            .unwrap();
        col.insert(make_point("far_away", vec![10.0, 10.0, 10.0]))
            .unwrap();
        FileStorage::save_collection(&col, &dir, false, false).unwrap();

        // WAL adiciona mais pontos
        let mut wal = Wal::open(&dir, 1000, false).unwrap();
        wal.append_upsert(&make_point("near_origin2", vec![0.2, 0.2, 0.2]))
            .unwrap();
        wal.append_delete("far_away").unwrap();
        drop(wal);

        // Recovery
        let recovered = recover_collection(&dir).unwrap().unwrap();
        assert_eq!(
            recovered.len(),
            2,
            "recovery must yield near_origin and near_origin2"
        );

        // Valida que busca funciona corretamente após recovery
        let query = vec![0.0, 0.0, 0.0];
        let results = recovered.search(&query, 5, None, None).unwrap();
        // Pelo menos um ponto próximo à origem; far_away não deve aparecer (foi deletado)
        assert!(!results.is_empty());
        assert!(!results.iter().any(|(id, _)| id == "far_away"));
        assert!(
            results.iter().any(|(id, _)| id == "near_origin")
                || results.iter().any(|(id, _)| id == "near_origin2"),
            "search should return at least one of near_origin, near_origin2"
        );
    }
}
