//! # Audit Routes — consulta de auditoria (Admin)

use axum::{routing::get, Router};

use crate::handlers::audit::get_audit;
use crate::state::AppState;

/// Rotas de auditoria (requerem Admin).
pub fn create_audit_routes() -> Router<AppState> {
    Router::new().route("/api/v1/audit", get(get_audit))
}
