//! # API Keys Routes — listar, criar e deletar chaves de API

use axum::{routing::delete, routing::get, Router};

use crate::handlers::keys::{create_key, delete_key, list_keys};
use crate::state::AppState;

/// Rotas de gerenciamento de API keys (protegidas por API key).
pub fn create_keys_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/keys", get(list_keys).post(create_key))
        .route("/api/v1/keys/{id}", delete(delete_key))
}
