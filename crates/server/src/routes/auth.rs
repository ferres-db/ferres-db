//! # Auth Routes — login do dashboard (público)

use axum::{routing::post, Router};

use crate::handlers::auth::login;
use crate::state::AppState;

/// Rotas de autenticação (sem middleware de API key).
pub fn create_auth_routes() -> Router<AppState> {
    Router::new().route("/api/v1/auth/login", post(login))
}
