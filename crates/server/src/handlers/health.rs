//! # Health Handlers
//!
//! Dois endpoints distintos seguindo o padrão Kubernetes dual-probe:
//!
//! - `GET /health` — **liveness**: só atomic loads + uptime. Timeout seguro de 1s.
//!   Usado pelo `HEALTHCHECK` do Dockerfile e pela `livenessProbe` do Kubernetes.
//!   Nunca toca disco, locks de coleção ou sysinfo.
//!
//! - `GET /ready` — **readiness**: verifica coleções (dirty count via read-locks),
//!   memória RSS e espaço em disco. Pode levar dezenas de ms. Usado pela
//!   `readinessProbe` do Kubernetes e pelo health gate do load balancer.

use std::path::Path;

use axum::{extract::State, response::Json};
use serde_json::json;
use sysinfo::{get_current_pid, System};

use crate::state::AppState;

/// GET /health — liveness probe (sempre barato, < 1 ms).
pub async fn health_check(State(app_state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "OK",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": app_state.started_at.elapsed().as_secs(),
    }))
}

/// GET /ready — readiness probe (pode tocar locks e disco).
pub async fn ready_check(State(app_state): State<AppState>) -> Json<serde_json::Value> {
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
        "uptime_seconds": app_state.started_at.elapsed().as_secs(),
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

fn get_memory_usage_mb() -> u64 {
    let sys = System::new_all();
    get_current_pid()
        .ok()
        .and_then(|pid| sys.process(pid))
        .map(|p| p.memory() / (1024 * 1024))
        .unwrap_or(0)
}

fn get_disk_free_mb(path: &Path) -> Option<u64> {
    fs4::available_space(path)
        .ok()
        .map(|bytes| bytes / (1024 * 1024))
}
