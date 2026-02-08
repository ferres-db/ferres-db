//! # Audit — audit trail completo para todas as operações
//!
//! Registra ações de usuários em arquivos JSONL com rotação diária.
//! O logger usa um channel buffered com uma única task de background para
//! reduzir overhead de I/O (batch writes em vez de um syscall por entry).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use std::sync::Mutex;

/// Resultado de uma ação auditada.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuditResult {
    /// Ação executada com sucesso.
    Success,
    /// Ação negada por falta de permissão.
    Denied,
    /// Ação falhou com erro.
    Error,
}

/// Entrada de auditoria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Timestamp UTC da ação.
    pub timestamp: DateTime<Utc>,
    /// Identificação do usuário (username ou "api_key").
    pub user_id: String,
    /// Ação executada (ex: "search", "upsert", "delete", "create_collection").
    pub action: String,
    /// Recurso afetado (ex: "collection:docs", "user:admin").
    pub resource: String,
    /// Detalhes resumidos do payload (sem vetores completos).
    pub details: serde_json::Value,
    /// Resultado da ação.
    pub result: AuditResult,
    /// Endereço IP do cliente (se disponível).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    /// Duração da operação em milissegundos (se disponível).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Tamanho máximo do buffer antes de forçar flush.
const FLUSH_BATCH_SIZE: usize = 100;

/// Logger de auditoria que escreve em arquivos JSONL com rotação diária.
///
/// Thread-safe e síncrono. Usa um channel buffered com uma única task de
/// background que acumula entries e faz flush por batch ou timeout (1s).
/// Cada chamada a `log()` é non-blocking (fire-and-forget via `try_send`).
pub struct AuditLogger {
    log_dir: PathBuf,
    /// Canal para enviar entries para a task de background.
    /// Wrapped in Mutex<Option<>> para permitir shutdown (drop do sender fecha o channel).
    tx: Arc<Mutex<Option<mpsc::Sender<AuditEntry>>>>,
    /// Handle da task de background (para shutdown graceful).
    bg_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// Estado interno do writer com referência ao arquivo e data corrente.
struct AuditWriter {
    file: Option<tokio::fs::File>,
    current_date: Option<String>,
    log_dir: PathBuf,
}

impl AuditWriter {
    fn new(log_dir: PathBuf) -> Self {
        Self {
            file: None,
            current_date: None,
            log_dir,
        }
    }

    /// Retorna o nome do arquivo de audit para uma data.
    fn file_name(date: &str) -> String {
        format!("audit-{date}.jsonl")
    }

    /// Faz flush de um batch de entries para disco em uma única escrita.
    async fn flush(&mut self, entries: &[AuditEntry]) {
        if entries.is_empty() {
            return;
        }

        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Rotação diária: se a data mudou, fecha o arquivo atual e abre novo
        let needs_rotate = self
            .current_date
            .as_ref()
            .map(|d| d != &today)
            .unwrap_or(true);

        if needs_rotate {
            // Fecha arquivo anterior (drop automático)
            self.file = None;

            let file_path = self.log_dir.join(Self::file_name(&today));
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .await
            {
                Ok(file) => {
                    self.file = Some(file);
                    self.current_date = Some(today);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open audit log file");
                    return;
                }
            }
        }

        if let Some(ref mut file) = self.file {
            // Serializa todas as entries de uma vez em um único buffer
            let mut buf = String::new();
            for entry in entries {
                match serde_json::to_string(entry) {
                    Ok(json) => {
                        buf.push_str(&json);
                        buf.push('\n');
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to serialize audit entry");
                    }
                }
            }
            if !buf.is_empty() {
                if let Err(e) = file.write_all(buf.as_bytes()).await {
                    tracing::warn!(error = %e, "failed to write audit entries batch");
                }
            }
        }
    }
}

impl AuditLogger {
    /// Cria um novo AuditLogger que escreve no diretório `log_dir`.
    ///
    /// Os arquivos são nomeados `audit-YYYY-MM-DD.jsonl`.
    /// Spawna uma task de background que recebe entries via channel e faz
    /// flush batched (a cada 100 entries ou 1 segundo, o que vier primeiro).
    pub fn new(log_dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&log_dir)?;

        let (tx, rx) = mpsc::channel::<AuditEntry>(10_000);

        let writer_log_dir = log_dir.clone();
        let bg_handle = tokio::spawn(Self::background_writer(rx, writer_log_dir));

        Ok(Self {
            log_dir,
            tx: Arc::new(Mutex::new(Some(tx))),
            bg_handle: Arc::new(tokio::sync::Mutex::new(Some(bg_handle))),
        })
    }

    /// Task de background que consome entries do channel e faz flush batched.
    async fn background_writer(mut rx: mpsc::Receiver<AuditEntry>, log_dir: PathBuf) {
        let mut writer = AuditWriter::new(log_dir);
        let mut buffer: Vec<AuditEntry> = Vec::with_capacity(FLUSH_BATCH_SIZE);
        let mut flush_interval = tokio::time::interval(std::time::Duration::from_secs(1));
        // O primeiro tick completa imediatamente; consumimos para não fazer flush vazio.
        flush_interval.tick().await;

        loop {
            tokio::select! {
                maybe_entry = rx.recv() => {
                    match maybe_entry {
                        Some(entry) => {
                            buffer.push(entry);
                            if buffer.len() >= FLUSH_BATCH_SIZE {
                                writer.flush(&buffer).await;
                                buffer.clear();
                            }
                        }
                        None => {
                            // Channel fechado — flush final e encerra
                            if !buffer.is_empty() {
                                writer.flush(&buffer).await;
                                buffer.clear();
                            }
                            break;
                        }
                    }
                }
                _ = flush_interval.tick() => {
                    if !buffer.is_empty() {
                        writer.flush(&buffer).await;
                        buffer.clear();
                    }
                }
            }
        }
    }

    /// Retorna o nome do arquivo de audit para uma data.
    fn file_name(date: &str) -> String {
        format!("audit-{date}.jsonl")
    }

    /// Registra uma entrada de auditoria (síncrono, non-blocking, fire-and-forget).
    ///
    /// Envia a entry para o channel buffered. Se o canal estiver cheio, loga
    /// um warning e descarta a entry (não bloqueia o caller).
    pub fn log(&self, entry: &AuditEntry) {
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.as_ref() {
            if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(entry.clone()) {
                tracing::warn!("audit channel full, dropping entry");
            }
            // TrySendError::Closed é silenciosamente ignorado (shutdown em curso)
        }
    }

    /// Fecha o channel e aguarda a task de background fazer o flush final.
    ///
    /// Deve ser chamado durante o shutdown graceful para garantir que
    /// nenhuma entry pendente seja perdida.
    pub async fn shutdown(&self) {
        // Drop do sender fecha o channel — a task de background recebe None e faz flush final.
        // Todos os clones de AuditLogger compartilham o mesmo Arc<Mutex<Option<Sender>>>,
        // então basta tomar o sender de um deles.
        {
            let mut guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            *guard = None;
        }
        // Aguarda a task de background terminar (flush final)
        let mut handle = self.bg_handle.lock().await;
        if let Some(h) = handle.take() {
            let _ = h.await;
        }
    }

    /// Consulta entradas de auditoria filtradas.
    ///
    /// Lê arquivos do período `from..to` e filtra por user/action/resource.
    /// Retorna no máximo `limit` entradas, mais recentes primeiro.
    pub async fn query(
        &self,
        user: Option<&str>,
        action: Option<&str>,
        resource: Option<&str>,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Vec<AuditEntry> {
        let from_date = from.unwrap_or_else(|| Utc::now() - chrono::Duration::days(7));
        let to_date = to.unwrap_or_else(Utc::now);

        // Coleta os arquivos de audit no range de datas
        let mut entries = Vec::new();

        let mut current = from_date.date_naive();
        let end = to_date.date_naive();

        while current <= end {
            let date_str = current.format("%Y-%m-%d").to_string();
            let file_path = self.log_dir.join(Self::file_name(&date_str));

            if file_path.exists() {
                if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
                    for line in content.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(entry) = serde_json::from_str::<AuditEntry>(line) {
                            // Filtro por timestamp
                            if entry.timestamp < from_date || entry.timestamp > to_date {
                                continue;
                            }
                            // Filtro por user
                            if let Some(u) = user {
                                if entry.user_id != u {
                                    continue;
                                }
                            }
                            // Filtro por action
                            if let Some(a) = action {
                                if entry.action != a {
                                    continue;
                                }
                            }
                            // Filtro por resource
                            if let Some(r) = resource {
                                if !entry.resource.contains(r) {
                                    continue;
                                }
                            }
                            entries.push(entry);
                        }
                    }
                }
            }

            current += chrono::Duration::days(1);
        }

        // Mais recentes primeiro
        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        entries.truncate(limit);
        entries
    }
}

impl Clone for AuditLogger {
    fn clone(&self) -> Self {
        Self {
            log_dir: self.log_dir.clone(),
            tx: self.tx.clone(),
            bg_handle: self.bg_handle.clone(),
        }
    }
}

/// Helper para criar um AuditEntry rapidamente.
pub fn audit_entry(
    user_id: &str,
    action: &str,
    resource: &str,
    details: serde_json::Value,
    result: AuditResult,
    ip_address: Option<String>,
    duration_ms: Option<u64>,
) -> AuditEntry {
    AuditEntry {
        timestamp: Utc::now(),
        user_id: user_id.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        details,
        result,
        ip_address,
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_logger_write_and_query() {
        let temp_dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();

        let entry = audit_entry(
            "testuser",
            "search",
            "collection:docs",
            serde_json::json!({"limit": 10}),
            AuditResult::Success,
            Some("127.0.0.1".to_string()),
            Some(5),
        );

        logger.log(&entry);

        // Shutdown flushes all pending entries
        logger.shutdown().await;

        // Re-create logger to query (original tx is closed)
        let logger2 = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();
        let results = logger2.query(
            Some("testuser"),
            Some("search"),
            None,
            None,
            None,
            100,
        ).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "testuser");
        assert_eq!(results[0].action, "search");
        assert_eq!(results[0].result, AuditResult::Success);
    }

    #[tokio::test]
    async fn test_audit_entry_serialization() {
        let entry = audit_entry(
            "admin",
            "create_collection",
            "collection:new-coll",
            serde_json::json!({"dimension": 384}),
            AuditResult::Success,
            None,
            Some(12),
        );

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.user_id, "admin");
        assert_eq!(deserialized.action, "create_collection");
    }

    #[tokio::test]
    async fn test_audit_query_filter_by_action() {
        let temp_dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();

        let entry1 = audit_entry("user1", "search", "collection:docs", serde_json::json!({}), AuditResult::Success, None, None);
        let entry2 = audit_entry("user1", "upsert", "collection:docs", serde_json::json!({}), AuditResult::Success, None, None);
        let entry3 = audit_entry("user2", "search", "collection:other", serde_json::json!({}), AuditResult::Denied, None, None);

        logger.log(&entry1);
        logger.log(&entry2);
        logger.log(&entry3);

        // Shutdown flushes all pending entries
        logger.shutdown().await;

        // Re-create logger to query
        let logger2 = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();

        // Filter by action "search"
        let results = logger2.query(None, Some("search"), None, None, None, 100).await;
        assert_eq!(results.len(), 2);

        // Filter by user "user1"
        let results = logger2.query(Some("user1"), None, None, None, None, 100).await;
        assert_eq!(results.len(), 2);

        // Filter by action "upsert"
        let results = logger2.query(None, Some("upsert"), None, None, None, 100).await;
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_audit_buffered_write() {
        // Envia 1000 entries rapidamente, verifica que todas aparecem no arquivo após flush.
        let temp_dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();

        for i in 0..1000 {
            let entry = audit_entry(
                &format!("user-{i}"),
                "search",
                &format!("collection:coll-{i}"),
                serde_json::json!({"index": i}),
                AuditResult::Success,
                None,
                None,
            );
            logger.log(&entry);
        }

        // Shutdown faz o flush final
        logger.shutdown().await;

        // Lê o arquivo JSONL diretamente para contar as linhas
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let file_path = temp_dir.path().join(format!("audit-{today}.jsonl"));
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1000, "all 1000 entries must be persisted");

        // Verifica que cada linha é um JSON válido de AuditEntry
        for line in &lines {
            let entry: AuditEntry = serde_json::from_str(line)
                .expect("each line must be a valid AuditEntry JSON");
            assert_eq!(entry.action, "search");
        }
    }

    #[tokio::test]
    async fn test_audit_flush_on_timeout() {
        // Envia 1 entry, espera 2 segundos, verifica que apareceu no arquivo
        // (o flush por timeout de 1s deve ter escrito antes dos 2s).
        let temp_dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::new(temp_dir.path().to_path_buf()).unwrap();

        let entry = audit_entry(
            "timeout-user",
            "login",
            "user:timeout-user",
            serde_json::json!({}),
            AuditResult::Success,
            None,
            None,
        );
        logger.log(&entry);

        // Espera 2 segundos para o flush por timeout (interval de 1s)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Lê o arquivo JSONL diretamente (sem shutdown)
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let file_path = temp_dir.path().join(format!("audit-{today}.jsonl"));
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 1, "entry must be flushed by timeout");

        let persisted: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(persisted.user_id, "timeout-user");
        assert_eq!(persisted.action, "login");

        // Cleanup: shutdown gracefully
        logger.shutdown().await;
    }
}
