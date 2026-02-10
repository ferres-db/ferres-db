//! # Cache warmup no startup
//!
//! Lê as últimas queries do query_logger, reexecuta-as em background para carregar
//! os índices HNSW na RAM (Hot Tier) e popular o search_cache.

use std::sync::Arc;

use tracing::{info, instrument, warn};

use crate::state::AppState;

const WARMUP_QUERY_LIMIT: usize = 50;

/// Dispara a tarefa de warmup em background.
/// Lê as últimas 50 queries do log que possuem vetor e reexecuta-as para
/// aquecer índices HNSW e o search_cache.
pub fn spawn_warmup_task(app_state: AppState) {
    tokio::spawn(async move {
        run_warmup(app_state).await;
    });
}

#[instrument(skip(app_state), name = "warmup")]
async fn run_warmup(app_state: AppState) {
    let entries = app_state
        .query_log_cache
        .last_n_entries_for_warmup(WARMUP_QUERY_LIMIT);

    let total = entries.len();
    if total == 0 {
        info!("warmup: no queries with vector in log, skipping");
        return;
    }

    info!(total = total, "warmup: starting cache warmup");

    let app_state = Arc::new(app_state);
    let entries = Arc::new(entries);

    // Executa as buscas em spawn_blocking para não bloquear o runtime async.
    let result = tokio::task::spawn_blocking({
        let app_state = Arc::clone(&app_state);
        let entries = Arc::clone(&entries);
        move || {
            let mut ran = 0u32;
            let mut skipped = 0u32;
            for (i, e) in entries.iter().enumerate() {
                let vector = match &e.vector {
                    Some(v) if !v.is_empty() => v.as_slice(),
                    _ => {
                        skipped += 1;
                        continue;
                    }
                };
                let collection_name = &e.collection;
                let limit = e.limit.max(1);

                let Some(collection_arc) = app_state.collections.get(collection_name) else {
                    tracing::debug!(
                        collection = %collection_name,
                        "warmup: collection not found, skipping"
                    );
                    skipped += 1;
                    continue;
                };

                let guard = match collection_arc.read() {
                    Ok(g) => g,
                    Err(_) => {
                        skipped += 1;
                        continue;
                    }
                };

                if let Err(dim_err) = guard.validate_dimension(vector) {
                    tracing::debug!(
                        collection = %collection_name,
                        error = %dim_err,
                        "warmup: dimension mismatch, skipping"
                    );
                    skipped += 1;
                    continue;
                }

                if let Err(search_err) = guard.search(vector, limit, None, None) {
                    tracing::debug!(
                        collection = %collection_name,
                        error = %search_err,
                        "warmup: search failed, skipping"
                    );
                    skipped += 1;
                    continue;
                }

                ran += 1;
                info!(
                    progress = %format!("{}/{}", i + 1, entries.len()),
                    collection = %collection_name,
                    "warmup: ran query"
                );
            }
            (ran, skipped)
        }
    })
    .await;

    match result {
        Ok((ran, skipped)) => {
            info!(
                ran = ran,
                skipped = skipped,
                total = total,
                "warmup: cache warmup completed"
            );
        }
        Err(e) => {
            warn!(error = %e, "warmup: task panicked or was cancelled");
        }
    }
}
