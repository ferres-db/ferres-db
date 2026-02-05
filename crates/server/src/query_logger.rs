//! # Query Logger — logging estruturado de queries de busca
//!
//! Registra todas as queries de busca em arquivo queries.log com formato JSON.

use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Estrutura de log de uma query.
#[derive(Debug, Serialize)]
struct QueryLogEntry {
    timestamp: String,
    collection: String,
    vector_preview: String,
    limit: usize,
    filter: Option<serde_json::Value>,
    results_count: usize,
    took_ms: u64,
}

/// Logger de queries que escreve em arquivo.
pub struct QueryLogger {
    log_file: PathBuf,
    writer: Arc<Mutex<Option<tokio::fs::File>>>,
}

impl QueryLogger {
    /// Cria um novo QueryLogger.
    pub fn new(log_dir: PathBuf) -> std::io::Result<Self> {
        // Garante que o diretório existe
        std::fs::create_dir_all(&log_dir)?;
        
        let log_file = log_dir.join("queries.log");
        
        Ok(Self {
            log_file,
            writer: Arc::new(Mutex::new(None)),
        })
    }

    /// Loga uma query de busca.
    pub async fn log_query(
        &self,
        collection: &str,
        vector: &[f32],
        limit: usize,
        filter: Option<&serde_json::Value>,
        results_count: usize,
        took_ms: u64,
    ) {
        // Cria preview do vetor (primeiros 3 valores)
        let vector_preview = if vector.len() >= 3 {
            format!("[{:.4}, {:.4}, {:.4}, ...]", vector[0], vector[1], vector[2])
        } else {
            format!("{:?}", vector)
        };

        let entry = QueryLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            collection: collection.to_string(),
            vector_preview,
            limit,
            filter: filter.cloned(),
            results_count,
            took_ms,
        };

        // Serializa para JSON
        let json = match serde_json::to_string(&entry) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize query log entry");
                return;
            }
        };

        // Abre o arquivo se necessário (lazy initialization)
        let mut writer_guard = self.writer.lock().await;
        if writer_guard.is_none() {
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_file)
                .await
            {
                Ok(file) => {
                    *writer_guard = Some(file);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open query log file");
                    return;
                }
            }
        }

        // Escreve no arquivo (com newline)
        if let Some(ref mut file) = *writer_guard {
            let line = format!("{}\n", json);
            if let Err(e) = file.write_all(line.as_bytes()).await {
                tracing::warn!(error = %e, "failed to write query log entry");
            }
        }
    }
}

impl Clone for QueryLogger {
    fn clone(&self) -> Self {
        Self {
            log_file: self.log_file.clone(),
            writer: self.writer.clone(),
        }
    }
}

