//! # Query Logger — logging estruturado de queries de busca
//!
//! Registra todas as queries de busca em arquivo queries.log com formato JSON.
//! Usa um canal MPSC bufferizado e uma task de background para não bloquear
//! o caminho crítico de busca com I/O de disco.

use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Estrutura de log de uma query.
/// O campo `vector` é opcional; quando presente, permite warmup ao reiniciar (replay das últimas queries).
#[derive(Debug, Serialize)]
struct QueryLogEntry {
    query_id: String,
    timestamp: String,
    collection: String,
    vector_preview: String,
    /// Vetor completo (opcional); usado pelo warmup para reexecutar a query e popular search_cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    vector: Option<Vec<f32>>,
    limit: usize,
    filter: Option<serde_json::Value>,
    results_count: usize,
    took_ms: u64,
}

/// Logger de queries que escreve em arquivo de forma assíncrona.
pub struct QueryLogger {
    tx: mpsc::Sender<QueryLogEntry>,
}

impl QueryLogger {
    /// Cria um novo QueryLogger e inicia a task de background.
    pub fn new(log_dir: PathBuf) -> std::io::Result<Self> {
        // Garante que o diretório existe
        std::fs::create_dir_all(&log_dir)?;
        let log_file_path = log_dir.join("queries.log");

        // Canal com buffer limitado para não estourar memória se o disco for lento
        let (tx, mut rx) = mpsc::channel::<QueryLogEntry>(4096);

        // Task de background para escrita em lote
        std::thread::Builder::new()
            .name("query-logger".into())
            .spawn(move || {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_file_path);

                let mut writer = match file {
                    Ok(f) => std::io::BufWriter::with_capacity(64 * 1024, f),
                    Err(e) => {
                        error!(path = %log_file_path.display(), error = %e, "failed to open query log file");
                        return;
                    }
                };

                let mut batch = Vec::with_capacity(64);

                // Loop de processamento
                while let Some(entry) = rx.blocking_recv() {
                   batch.push(entry);

                   // Tenta drena mais itens do canal sem bloquear, até um limite
                   while batch.len() < 100 {
                       match rx.try_recv() {
                           Ok(e) => batch.push(e),
                           Err(_) => break,
                       }
                   }

                   // Escreve o batch no buffer
                   for entry in batch.drain(..) {
                       if let Ok(json) = serde_json::to_string(&entry) {
                           if let Err(e) = writeln!(writer, "{}", json) {
                               error!(error = %e, "failed to write query log entry");
                           }
                       }
                   }

                   // Flush periódico (implícito pelo BufWriter, mas forçamos para garantir durabilidade razoável)
                   if let Err(e) = writer.flush() {
                       error!(error = %e, "failed to flush query log file");
                   }
                }

                info!("query logger background thread stopped");
            })?;

        Ok(Self { tx })
    }

    /// Loga uma query de busca.
    /// Se `query_id` for `Some`, usa esse id; caso contrário gera um novo UUID.
    /// O vetor completo é gravado opcionalmente para permitir warmup no próximo startup.
    ///
    /// Esta função é non-blocking: se o buffer do canal estiver cheio, o log é descartado
    /// para não impactar a latência da busca.
    #[allow(clippy::too_many_arguments)]
    pub fn log_query(
        &self,
        query_id: Option<&str>,
        collection: &str,
        vector: &[f32],
        limit: usize,
        filter: Option<&serde_json::Value>,
        results_count: usize,
        took_ms: u64,
    ) {
        // Cria preview do vetor (primeiros 3 valores)
        let vector_preview = if vector.len() >= 3 {
            format!(
                "[{:.4}, {:.4}, {:.4}, ...]",
                vector[0], vector[1], vector[2]
            )
        } else {
            format!("{vector:?}")
        };

        let entry = QueryLogEntry {
            query_id: query_id
                .map(String::from)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            timestamp: Utc::now().to_rfc3339(),
            collection: collection.to_string(),
            vector_preview,
            vector: Some(vector.to_vec()),
            limit,
            filter: filter.cloned(),
            results_count,
            took_ms,
        };

        // Envia para o canal. Se cheio, descarta silenciosamente (ou loga warn com rate limit, mas aqui simplificamos)
        // try_send é crucial para não bloquear o thread de runtime do axum
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(entry) {
            // Em alta carga extremos, preferimos perder logs a travar o servidor
            // Podemos adicionar uma métrica de "dropped_logs" futuramente
            warn!("query log buffer full, dropping entry");
        }
    }
}
