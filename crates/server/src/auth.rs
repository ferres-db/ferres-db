//! # Auth — autenticação via API Key ou JWT (dashboard)
//!
//! Middleware aceita: API key (Bearer <key>) ou JWT (Bearer <jwt>).
//! JWT é usado após login do dashboard (usuários em SQLite).

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::sync::OnceLock;

static API_KEYS_LEGACY: OnceLock<HashSet<String>> = OnceLock::new();
static JWT_SECRET: OnceLock<Vec<u8>> = OnceLock::new();

/// Chama no main para que o middleware e o login handler possam usar o secret.
pub fn set_jwt_secret(secret: Vec<u8>) {
    let _ = JWT_SECRET.set(secret);
}

pub fn get_jwt_secret() -> Option<&'static [u8]> {
    JWT_SECRET.get().map(|v| v.as_slice())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,   // username
    pub role: String,  // admin | editor | viewer
    pub exp: i64,
    pub iat: i64,
}

/// Informação do usuário autenticado (colocada em request.extensions pelo middleware).
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub username: String,
    pub role: crate::users::Role,
}

fn looks_like_jwt(token: &str) -> bool {
    token.split('.').count() == 3
}

fn parse_keys(raw: &str) -> HashSet<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Inicializa API keys a partir de uma string (legacy: config.toml ou env).
/// Usado apenas em testes; em produção as chaves vêm do SQLite via ApiKeyStore::init.
pub fn init_api_keys_from(raw: Option<&str>) {
    API_KEYS_LEGACY.get_or_init(|| parse_keys(raw.unwrap_or_default()));
}

/// Inicializa API keys da variável de ambiente (legacy, para testes).
pub fn init_api_keys() {
    let env_val = std::env::var("FERRESDB_API_KEYS").unwrap_or_default();
    init_api_keys_from(Some(&env_val));
}

fn is_valid_legacy(key: &str) -> bool {
    API_KEYS_LEGACY
        .get()
        .map(|keys| keys.contains(key))
        .unwrap_or(false)
}

/// Middleware que requer API key válida no header `Authorization: Bearer <key>`.
/// Aceita chaves do SQLite (api_keys) ou do legacy static (testes).
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
                Json(json!({
                    "message": "Missing or invalid Authorization header. Use: Authorization: Bearer <api-key>",
                    "code": "unauthorized"
                })),
            ));
        }
    };

    use crate::users::Role;

    // 1) API key (programática ou legacy) → full access (admin)
    if crate::api_keys::ApiKeyStore::validate(api_key) || is_valid_legacy(api_key) {
        let mut req = req;
        req.extensions_mut().insert(AuthUser {
            username: "api_key".to_string(),
            role: Role::Admin,
        });
        return Ok(next.run(req).await);
    }

    // 2) JWT (login do dashboard)
    if looks_like_jwt(api_key) {
        if let Some(secret) = JWT_SECRET.get() {
            let mut validation = Validation::default();
            validation.validate_exp = true;
            if let Ok(token_data) =
                decode::<JwtClaims>(api_key, &DecodingKey::from_secret(secret), &validation)
            {
                let role = Role::from_str(&token_data.claims.role).unwrap_or(Role::Viewer);
                let mut req = req;
                req.extensions_mut().insert(AuthUser {
                    username: token_data.claims.sub.clone(),
                    role,
                });
                return Ok(next.run(req).await);
            }
        }
    }

    Err((
        StatusCode::FORBIDDEN,
        Json(json!({
            "message": "Invalid API key or session. Use a valid API key or log in to the dashboard.",
            "code": "forbidden"
        })),
    ))
}

fn forbidden_role() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "message": "Insufficient permissions for this action.",
            "code": "forbidden"
        })),
    )
}

/// Extrator que exige role Admin. Use em handlers restritos a administradores.
#[derive(Debug, Clone, Copy)]
pub struct RequireAdmin;

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let auth = parts
            .extensions
            .get::<AuthUser>()
            .ok_or_else(forbidden_role)?;
        if auth.role >= crate::users::Role::Admin {
            Ok(RequireAdmin)
        } else {
            Err(forbidden_role())
        }
    }
}

/// Extrator que exige role Editor ou superior. Use em handlers de criação/edição (coleções, API keys, etc.).
#[derive(Debug, Clone, Copy)]
pub struct RequireEditor;

impl<S> FromRequestParts<S> for RequireEditor
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let auth = parts
            .extensions
            .get::<AuthUser>()
            .ok_or_else(forbidden_role)?;
        if auth.role >= crate::users::Role::Editor {
            Ok(RequireEditor)
        } else {
            Err(forbidden_role())
        }
    }
}
