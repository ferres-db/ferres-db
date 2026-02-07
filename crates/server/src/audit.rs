//! # Audit — audit trail completo para todas as operações
//!
//! Registra ações de usuários em arquivos JSONL com rotação diária.
//! O logger é async e não bloqueia handlers (usa `tokio::spawn`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

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

/// Logger de auditoria que escreve em arquivos JSONL com rotação diária.
///
/// Thread-safe e async. Uso: clone o Arc<AuditLogger> e chame `log()` via `tokio::spawn`.
pub struct AuditLogger {
    log_dir: PathBuf,
    /// Writer corrente (arquivo do dia). Protegido por Mutex para escrita atômica.
    writer: Arc<Mutex<AuditWriter>>,
}

/// Estado interno do writer com referência ao arquivo e data corrente.
struct AuditWriter {
    file: Option<tokio::fs::File>,
    current_date: Option<String>,
}

impl AuditLogger {
    /// Cria um novo AuditLogger que escreve no diretório `log_dir`.
    ///
    /// Os arquivos são nomeados `audit-YYYY-MM-DD.jsonl`.
    pub fn new(log_dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&log_dir)?;
        Ok(Self {
            log_dir,
            writer: Arc::new(Mutex::new(AuditWriter {
                file: None,
                current_date: None,
            })),
        })
    }

    /// Retorna o nome do arquivo de audit para uma data.
    fn file_name(date: &str) -> String {
        format!("audit-{}.jsonl", date)
    }

    /// Registra uma entrada de auditoria (async, não bloqueia o caller).
    pub async fn log(&self, entry: &AuditEntry) {
        let json = match serde_json::to_string(entry) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize audit entry");
                return;
            }
        };

        let today = Utc::now().format("%Y-%m-%d").to_string();

        let mut writer = self.writer.lock().await;

        // Rotação diária: se a data mudou, fecha o arquivo atual e abre novo
        let needs_rotate = writer
            .current_date
            .as_ref()
            .map(|d| d != &today)
            .unwrap_or(true);

        if needs_rotate {
            // Fecha arquivo anterior (drop automático)
            writer.file = None;

            let file_path = self.log_dir.join(Self::file_name(&today));
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .await
            {
                Ok(file) => {
                    writer.file = Some(file);
                    writer.current_date = Some(today);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open audit log file");
                    return;
                }
            }
        }

        if let Some(ref mut file) = writer.file {
            let line = format!("{}\n", json);
            if let Err(e) = file.write_all(line.as_bytes()).await {
                tracing::warn!(error = %e, "failed to write audit entry");
            }
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
            writer: self.writer.clone(),
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

        logger.log(&entry).await;

        // Flush by dropping writer lock
        let results = logger.query(
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

        logger.log(&entry1).await;
        logger.log(&entry2).await;
        logger.log(&entry3).await;

        // Filter by action "search"
        let results = logger.query(None, Some("search"), None, None, None, 100).await;
        assert_eq!(results.len(), 2);

        // Filter by user "user1"
        let results = logger.query(Some("user1"), None, None, None, None, 100).await;
        assert_eq!(results.len(), 2);

        // Filter by action "upsert"
        let results = logger.query(None, Some("upsert"), None, None, None, 100).await;
        assert_eq!(results.len(), 1);
    }
}
