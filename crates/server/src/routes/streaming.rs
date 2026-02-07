//! # Streaming Routes — rotas WebSocket para ingestão em tempo real

use axum::{routing::get, Router};

use crate::handlers::streaming::ws_handler;
use crate::state::AppState;

/// Cria a rota WebSocket para streaming.
pub fn create_streaming_routes() -> Router<AppState> {
    Router::new().route("/api/v1/ws", get(ws_handler))
}
