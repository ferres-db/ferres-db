//! # Health Handlers — handlers para health check

use std::path::Path;

use axum::{extract::State, response::Json};
use serde_json::json;
use sysinfo::{get_current_pid, System};

use crate::state::AppState;

/// Retorna uso de memória do processo atual em MB (RSS).
/// Em falha (ex.: plataforma não suportada), retorna 0.
fn get_memory_usage_mb() -> u64 {
    let sys = System::new_all();
    get_current_pid()
        .ok()
        .and_then(|pid| sys.process(pid))
        .map(|p| p.memory() / (1024 * 1024))
        .unwrap_or(0)
}

/// Retorna espaço livre em disco no sistema de arquivos do path, em MB.
/// Retorna `None` se a operação falhar (path inexistente, permissão, etc.).
fn get_disk_free_mb(path: &Path) -> Option<u64> {
    fs4::available_space(path)
        .ok()
        .map(|bytes| bytes / (1024 * 1024))
}

/// Handler para GET /health
///
/// Retorna status detalhado para liveness/readiness no Kubernetes:
/// versão, uptime, coleções (total e dirty), memória e disco livre.
pub async fn health_check(State(app_state): State<AppState>) -> Json<serde_json::Value> {
    let uptime = app_state.started_at.elapsed();
    let total = app_state.collections.len();
    let mut dirty_count: usize = 0;
    for entry in app_state.collections.iter() {
        if let Ok(guard) = entry.value().read() {
            if guard.is_dirty() {
                dirty_count += 1;
            }
        }
    }
    let memory_mb = get_memory_usage_mb();
    let disk_free_mb = get_disk_free_mb(&app_state.config.storage_path);

    let mut body = json!({
        "status": "OK",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": uptime.as_secs(),
        "collections": {
            "total": total,
            "dirty": dirty_count,
        },
        "memory_mb": memory_mb,
    });
    if let Some(free) = disk_free_mb {
        body["disk_free_mb"] = json!(free);
    }

    Json(body)
}
