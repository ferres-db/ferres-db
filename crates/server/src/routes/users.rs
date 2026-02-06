//! # Users Routes — listar, criar, remover e alterar senha (Admin)

use axum::{routing::delete, routing::get, routing::post, routing::put, Router};

use crate::handlers::users::{
    create_user, delete_user, list_users, update_user_password,
};
use crate::state::AppState;

/// Rotas de usuários (requerem Admin).
pub fn create_users_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{id}", delete(delete_user))
        .route("/api/v1/users/{username}/password", put(update_user_password))
}
