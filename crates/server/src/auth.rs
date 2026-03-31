//! # Auth — autenticação via API Key ou JWT (dashboard)
//!
//! Middleware aceita: API key (Bearer <key>) ou JWT (Bearer <jwt>).
//! JWT é usado após login do dashboard (usuários em SQLite).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
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

/// Valida um token JWT (ex.: do login do dashboard).
/// Usado pelo handler WebSocket para aceitar o mesmo token que o REST.
/// Retorna true se o token for um JWT válido (assinatura e expiração).
pub fn validate_jwt(token: &str) -> bool {
    let secret = match get_jwt_secret() {
        Some(s) => s,
        None => return false,
    };
    if !looks_like_jwt(token) {
        return false;
    }
    let mut validation = Validation::default();
    validation.validate_exp = true;
    decode::<JwtClaims>(token, &DecodingKey::from_secret(secret), &validation).is_ok()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,  // username
    pub role: String, // admin | editor | viewer
    pub exp: i64,
    pub iat: i64,
}

/// Informação do usuário autenticado (colocada em request.extensions pelo middleware).
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub username: String,
    pub role: crate::users::Role,
    /// Permissões granulares (RBAC). Se None, aplica-se comportamento legado baseado em role.
    pub permissions: Option<Vec<crate::permissions::Permission>>,
    /// Restrição de namespace (API key ou usuário). None = acesso a todos os namespaces.
    pub namespace_allowance: Option<crate::permissions::NamespaceAllowance>,
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
pub async fn require_api_key(req: Request, next: Next) -> Result<Response, impl IntoResponse> {
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

    // 1) API key (programática ou legacy) → full access (admin), com possível restrição de namespace
    if crate::api_keys::ApiKeyStore::validate(api_key) || is_valid_legacy(api_key) {
        let namespace_allowance = crate::api_keys::get_meta_global(api_key).and_then(|meta| {
            meta.allowed_namespaces
                .map(|list| crate::permissions::NamespaceAllowance::Only(list))
        });
        let user = AuthUser {
            username: "api_key".to_string(),
            role: Role::Admin,
            permissions: None,
            namespace_allowance,
        };
        // Validar namespace solicitado na query ou no header antes de prosseguir
        if let Err(resp) = check_request_namespace(&req, &user) {
            return Err(resp);
        }
        let mut req = req;
        req.extensions_mut().insert(user);
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
                let role = token_data.claims.role.parse().unwrap_or(Role::Viewer);

                // Carrega permissões granulares do UserStore (se disponível)
                // Admin bypassa: não precisa de permissões granulares
                let permissions = if role < Role::Admin {
                    // Tenta carregar permissões do state (AppState está no req extensions)
                    // Como o middleware não tem acesso direto ao State, usamos a
                    // abordagem de codificar as permissões no JWT claims.
                    // Fallback: sem permissões granulares = comportamento legado.
                    None
                } else {
                    None
                };

                let mut req = req;
                req.extensions_mut().insert(AuthUser {
                    username: token_data.claims.sub.clone(),
                    role,
                    permissions,
                    namespace_allowance: None, // JWT: sem restrição de namespace por chave
                });
                return Ok(next.run(req).await);
            }
        }
        // Token parece JWT mas falhou na validação (expirado ou assinatura inválida).
        // Retorna 401 para que o cliente limpe o token e redirecione para login.
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "message": "Session expired or invalid. Please log in again.",
                "code": "unauthorized"
            })),
        ));
    }

    Err((
        StatusCode::FORBIDDEN,
        Json(json!({
            "message": "Invalid API key. Use a valid API key or log in to the dashboard.",
            "code": "forbidden"
        })),
    ))
}

/// Extrai o namespace solicitado na request (query param `namespace` ou header `X-Namespace`).
fn requested_namespace_from_request(req: &Request) -> Option<String> {
    if let Some(q) = req.uri().query() {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            if k == "namespace" && !v.is_empty() {
                return Some(v.into_owned());
            }
        }
    }
    req.headers()
        .get("x-namespace")
        .or_else(|| req.headers().get("X-Namespace"))
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Valida se o usuário pode acessar o namespace solicitado na request (query/header).
/// Retorna Err(403) se a chave tem restrição de namespace e o namespace solicitado não está permitido.
fn check_request_namespace(
    req: &Request,
    user: &AuthUser,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let allowance = match &user.namespace_allowance {
        None => return Ok(()),
        Some(a) => a,
    };
    let requested = requested_namespace_from_request(req);
    if allowance.allows(requested.as_deref()) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "API key does not have access to the requested namespace.",
                "code": "forbidden_namespace"
            })),
        ))
    }
}

/// Verifica se o usuário pode acessar o namespace indicado (ex.: extraído do body).
/// Use em handlers que recebem namespace no body. Retorna Err com ApiError para 403.
pub fn check_namespace_access(
    user: &AuthUser,
    requested: Option<&str>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let allowance = match &user.namespace_allowance {
        None => return Ok(()),
        Some(a) => a,
    };
    if allowance.allows(requested) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "API key does not have access to this namespace.",
                "code": "forbidden_namespace"
            })),
        ))
    }
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

/// Extrator que obtém o AuthUser do request e enriquece com permissões do UserStore.
///
/// Uso: `AuthenticatedUser(user)` nos handlers que precisam das permissões granulares.
/// Requer que o router use `AppState` como state.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub AuthUser);

impl FromRequestParts<crate::state::AppState> for AuthenticatedUser {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::state::AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or_else(forbidden_role)?;

        // Se já tem permissões (ex: codificadas no JWT) ou é Admin, retorna direto
        if auth.permissions.is_some() || auth.role >= crate::users::Role::Admin {
            return Ok(AuthenticatedUser(auth));
        }

        // Carrega permissões granulares do UserStore (acesso ao state via Axum)
        if let Some(ref store) = state.user_store {
            if let Ok(Some(perms)) = store.get_permissions(&auth.username) {
                return Ok(AuthenticatedUser(AuthUser {
                    permissions: Some(perms),
                    ..auth
                }));
            }
        }

        // Sem permissões granulares configuradas: comportamento legado
        Ok(AuthenticatedUser(auth))
    }
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

// ─── Permission Checking Helpers ─────────────────────────────────────────

/// Verifica permissão granular para um usuário autenticado em uma collection.
///
/// Regras de precedência:
/// 1. Admin → sempre permitido (retorna Allowed)
/// 2. Se o usuário tem permissões granulares configuradas → usa check_permission
/// 3. Se não tem permissões granulares (legado) → usa a role (Editor pode Read/Write/Create,
///    Viewer pode Read)
///
/// Backward compatible: se nenhuma permissão granular está configurada, o
/// comportamento é idêntico ao sistema anterior.
pub fn check_user_permission(
    user: &AuthUser,
    collection: &str,
    action: &crate::permissions::Action,
) -> crate::permissions::PermissionResult {
    use crate::permissions::{Action, PermissionResult};
    use crate::users::Role;

    // Admin bypassa tudo
    if user.role >= Role::Admin {
        return PermissionResult::Allowed;
    }

    // Se tem permissões granulares, usa-as
    if let Some(ref perms) = user.permissions {
        if !perms.is_empty() {
            return crate::permissions::check_permission(perms, collection, action);
        }
    }

    // Fallback: comportamento legado baseado em role
    match user.role {
        Role::Editor => {
            // Editor pode Read, Write, Create (mas não Delete collection nem Admin)
            match action {
                Action::Read | Action::Write | Action::Create => PermissionResult::Allowed,
                Action::Delete | Action::Admin => PermissionResult::Denied(
                    "editors cannot delete collections or perform admin actions".to_string(),
                ),
            }
        }
        Role::Viewer => {
            // Viewer pode apenas Read
            match action {
                Action::Read => PermissionResult::Allowed,
                _ => PermissionResult::Denied(format!(
                    "viewers can only read; action '{action}' denied"
                )),
            }
        }
        _ => PermissionResult::Denied("insufficient permissions".to_string()),
    }
}

/// Helper para extrair IP do cliente a partir dos headers (X-Forwarded-For ou fallback).
pub fn extract_client_ip(parts: &Parts) -> Option<String> {
    // Tenta X-Forwarded-For primeiro (proxy/load balancer)
    if let Some(xff) = parts.headers.get("x-forwarded-for") {
        if let Ok(s) = xff.to_str() {
            return Some(s.split(',').next().unwrap_or(s).trim().to_string());
        }
    }
    // Tenta X-Real-IP
    if let Some(xri) = parts.headers.get("x-real-ip") {
        if let Ok(s) = xri.to_str() {
            return Some(s.trim().to_string());
        }
    }
    None
}
