//! # Auth — autenticação via API Key
//!
//! Middleware de autenticação que valida API keys no header `Authorization: Bearer <key>`.
//! As keys são carregadas da variável de ambiente `FERRESDB_API_KEYS` (separadas por vírgula).

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::HashSet;
use std::sync::OnceLock;

static API_KEYS: OnceLock<HashSet<String>> = OnceLock::new();

/// Inicializa API keys da variável de ambiente `FERRESDB_API_KEYS`.
///
/// As keys devem ser separadas por vírgula. Espaços em branco são removidos.
/// Exemplo: `FERRESDB_API_KEYS="sk-dev-abc123,sk-prod-xyz789"`
///
/// Seguro para chamar múltiplas vezes — usa `get_or_init` internamente.
pub fn init_api_keys() {
    API_KEYS.get_or_init(|| {
        std::env::var("FERRESDB_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
}

/// Middleware que requer API key válida no header `Authorization: Bearer <key>`.
///
/// Retorna 401 se o header estiver ausente/malformado, ou 403 se a key for inválida.
pub async fn require_api_key(
    req: Request,
    next: Next,
) -> Result<Response, impl IntoResponse> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    let api_key = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Missing or invalid Authorization header. Use: Authorization: Bearer <api-key>",
            ));
        }
    };

    let keys = API_KEYS.get().expect("API keys not initialized");
    if !keys.contains(api_key) {
        return Err((StatusCode::FORBIDDEN, "Invalid API key"));
    }

    Ok(next.run(req).await)
}
