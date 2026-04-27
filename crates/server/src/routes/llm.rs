//! # LLM routes
//!
//! - POST /api/v1/llm/complete — Editor+ proxy para OpenAI/Anthropic/Gemini.
//! - GET/PUT/DELETE /api/v1/admin/llm-credentials[/{provider}] — Admin CRUD de chaves.

use axum::routing::{get, post, put};
use axum::Router;

use crate::handlers::llm_credentials::{
    delete_llm_credential, list_llm_credentials, put_llm_credential,
};
use crate::handlers::llm_proxy::complete;
use crate::state::AppState;

pub fn create_llm_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/llm/complete", post(complete))
        .route(
            "/api/v1/admin/llm-credentials",
            get(list_llm_credentials),
        )
        .route(
            "/api/v1/admin/llm-credentials/{provider}",
            put(put_llm_credential).delete(delete_llm_credential),
        )
}
