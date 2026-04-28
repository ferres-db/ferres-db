//! # Error — tratamento de erros HTTP
//!
//! Define tipos de erro HTTP customizados e conversões de erros do core
//! para respostas HTTP apropriadas.

/// Converte um `Result` em `ApiError::InternalError` com contexto, para uso uniforme nos handlers.
///
/// Uso: `let guard = api_err!(rwlock.read(), "failed to acquire read lock")?;`
#[macro_export]
macro_rules! api_err {
    ($expr:expr, $context:expr) => {
        $expr.map_err(|e| $crate::error::ApiError::internal_error(format!("{}: {}", $context, e)))
    };
}

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;

use ferres_db_core::FerresError;

// ─── ApiError ───────────────────────────────────────────────────────────

/// Tipos de erro da API HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiError {
    /// Coleção não encontrada.
    CollectionNotFound {
        /// Mensagem de erro.
        message: String,
    },
    /// Coleção já existe (conflito).
    CollectionAlreadyExists {
        /// Mensagem de erro.
        message: String,
    },
    /// Dimensão inválida (validação de entrada).
    InvalidDimension {
        /// Mensagem de erro.
        message: String,
    },
    /// Payload inválido (JSON malformado, campos faltando, etc).
    InvalidPayload {
        /// Mensagem de erro.
        message: String,
    },
    /// Erro interno do servidor.
    InternalError {
        /// Mensagem de erro.
        message: String,
    },
    /// Perfil de query não encontrado (debug endpoint).
    QueryProfileNotFound {
        /// Mensagem de erro.
        message: String,
    },
    /// API key store não disponível (múltiplas chaves não configuradas).
    ApiKeyStoreUnavailable {
        /// Mensagem de erro.
        message: String,
    },
    /// Query excede o orçamento de latência (budget_ms).
    BudgetExceeded {
        /// Mensagem de erro.
        message: String,
        /// Estimativa de custo detalhada.
        estimate: serde_json::Value,
    },
    /// Permissão negada (RBAC).
    Forbidden {
        /// Mensagem de erro.
        message: String,
    },
    /// Método não permitido (ex.: escrita em nó réplica).
    MethodNotAllowed {
        /// Mensagem de erro.
        message: String,
    },
}

impl ApiError {
    /// Cria um erro de coleção não encontrada.
    pub fn collection_not_found(name: impl Into<String>) -> Self {
        Self::CollectionNotFound {
            message: format!("collection '{}' not found", name.into()),
        }
    }

    /// Cria um erro de ponto não encontrado.
    pub fn point_not_found(id: impl Into<String>) -> Self {
        Self::CollectionNotFound {
            message: format!("point '{}' not found", id.into()),
        }
    }

    /// Cria um erro de coleção já existente.
    pub fn collection_already_exists(name: impl Into<String>) -> Self {
        Self::CollectionAlreadyExists {
            message: format!("collection '{}' already exists", name.into()),
        }
    }

    /// Cria um erro de dimensão inválida.
    pub fn invalid_dimension(message: impl Into<String>) -> Self {
        Self::InvalidDimension {
            message: message.into(),
        }
    }

    /// Cria um erro de payload inválido.
    pub fn invalid_payload(message: impl Into<String>) -> Self {
        Self::InvalidPayload {
            message: message.into(),
        }
    }

    /// Cria um erro interno.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    /// Cria um erro de store de API keys indisponível.
    pub fn api_key_store_unavailable(message: impl Into<String>) -> Self {
        Self::ApiKeyStoreUnavailable {
            message: message.into(),
        }
    }

    /// Cria um erro de orçamento excedido (422 Unprocessable Entity).
    pub fn budget_exceeded(message: impl Into<String>, estimate: serde_json::Value) -> Self {
        Self::BudgetExceeded {
            message: message.into(),
            estimate,
        }
    }

    /// Cria um erro de permissão negada (403 Forbidden).
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
        }
    }

    /// Cria um erro de método não permitido (405 Method Not Allowed).
    pub fn method_not_allowed(message: impl Into<String>) -> Self {
        Self::MethodNotAllowed {
            message: message.into(),
        }
    }

    /// Retorna o código de status HTTP correspondente.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::CollectionNotFound { .. } | Self::QueryProfileNotFound { .. } => {
                StatusCode::NOT_FOUND
            }
            Self::CollectionAlreadyExists { .. } => StatusCode::CONFLICT,
            Self::InvalidDimension { .. } | Self::InvalidPayload { .. } => StatusCode::BAD_REQUEST,
            Self::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ApiKeyStoreUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::BudgetExceeded { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::MethodNotAllowed { .. } => StatusCode::METHOD_NOT_ALLOWED,
        }
    }

    /// Retorna o tipo do erro como string.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::CollectionNotFound { .. } => "collection_not_found",
            Self::CollectionAlreadyExists { .. } => "collection_already_exists",
            Self::InvalidDimension { .. } => "invalid_dimension",
            Self::InvalidPayload { .. } => "invalid_payload",
            Self::InternalError { .. } => "internal_error",
            Self::QueryProfileNotFound { .. } => "query_profile_not_found",
            Self::ApiKeyStoreUnavailable { .. } => "api_key_store_unavailable",
            Self::BudgetExceeded { .. } => "budget_exceeded",
            Self::Forbidden { .. } => "forbidden",
            Self::MethodNotAllowed { .. } => "method_not_allowed",
        }
    }

    /// Retorna a mensagem de erro.
    pub fn message(&self) -> &str {
        match self {
            Self::CollectionNotFound { message } => message,
            Self::CollectionAlreadyExists { message } => message,
            Self::InvalidDimension { message } => message,
            Self::InvalidPayload { message } => message,
            Self::InternalError { message } => message,
            Self::QueryProfileNotFound { message } => message,
            Self::ApiKeyStoreUnavailable { message } => message,
            Self::BudgetExceeded { message, .. } => message,
            Self::Forbidden { message } => message,
            Self::MethodNotAllowed { message } => message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let error_type = self.error_type();
        let message = self.message().to_string();

        // Loga o erro
        error!(
            status = %status.as_u16(),
            error_type = %error_type,
            message = %message,
            "API error"
        );

        // Para BudgetExceeded, inclui a estimativa no body para que o cliente possa analisar
        let body = match &self {
            Self::BudgetExceeded { estimate, .. } => {
                json!({
                    "error": error_type,
                    "message": message,
                    "code": status.as_u16(),
                    "estimate": estimate,
                })
            }
            _ => {
                json!({
                    "error": error_type,
                    "message": message,
                    "code": status.as_u16(),
                })
            }
        };

        (status, Json(body)).into_response()
    }
}

// ─── Conversões de FerresError ─────────────────────────────────────────

impl From<FerresError> for ApiError {
    fn from(err: FerresError) -> Self {
        match &err {
            FerresError::CollectionNotFound(name) => ApiError::collection_not_found(name.clone()),
            FerresError::CollectionAlreadyExists(name) => {
                ApiError::collection_already_exists(name.clone())
            }
            FerresError::PointNotFound(id) => {
                ApiError::collection_not_found(format!("point '{id}' not found"))
            }
            FerresError::DimensionMismatch { expected, got } => ApiError::invalid_dimension(
                format!("dimension mismatch: expected {expected}, got {got}"),
            ),
            FerresError::InvalidVector { reason } => {
                ApiError::invalid_dimension(format!("invalid vector: {reason}"))
            }
            FerresError::Storage(msg) => ApiError::internal_error(format!("storage error: {msg}")),
            FerresError::IndexNotBuilt => {
                ApiError::internal_error("index not built: call build() before searching")
            }
            FerresError::InvalidPointId(id) => {
                ApiError::invalid_payload(format!("invalid point id: {id}"))
            }
            FerresError::EmptyVector => ApiError::invalid_dimension("vector cannot be empty"),
            FerresError::UnknownVectorField(name) => {
                ApiError::invalid_payload(format!("unknown vector field: {name}"))
            }
        }
    }
}

impl From<crate::api_keys::ApiKeyError> for ApiError {
    fn from(err: crate::api_keys::ApiKeyError) -> Self {
        ApiError::internal_error(err.to_string())
    }
}

impl From<crate::users::UserError> for ApiError {
    fn from(err: crate::users::UserError) -> Self {
        use crate::users::UserError;
        match &err {
            UserError::DuplicateUsername => ApiError::invalid_payload("username already exists"),
            UserError::InvalidUsername => ApiError::invalid_payload("username is required"),
            UserError::InvalidPassword => ApiError::invalid_payload("password is required"),
            _ => ApiError::internal_error(err.to_string()),
        }
    }
}

// ─── Result type alias ────────────────────────────────────────────────────

/// Tipo de resultado conveniente para handlers HTTP.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::Value;

    /// Helper para extrair JSON de uma resposta.
    async fn extract_json(response: Response) -> Value {
        let (_parts, body) = response.into_parts();
        let body_bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        serde_json::from_slice(&body_bytes).unwrap()
    }

    #[tokio::test]
    async fn test_collection_not_found_status() {
        let error = ApiError::collection_not_found("test-collection");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_collection_not_found_json() {
        let error = ApiError::collection_not_found("test-collection");
        let response = error.into_response();
        let json: Value = extract_json(response).await;

        assert_eq!(json["error"], "collection_not_found");
        assert_eq!(json["message"], "collection 'test-collection' not found");
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn test_invalid_dimension_status() {
        let error = ApiError::invalid_dimension("dimension must be > 0");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_invalid_dimension_json() {
        let error = ApiError::invalid_dimension("dimension must be > 0");
        let response = error.into_response();
        let json: Value = extract_json(response).await;

        assert_eq!(json["error"], "invalid_dimension");
        assert_eq!(json["message"], "dimension must be > 0");
        assert_eq!(json["code"], 400);
    }

    #[tokio::test]
    async fn test_invalid_payload_status() {
        let error = ApiError::invalid_payload("invalid JSON format");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_invalid_payload_json() {
        let error = ApiError::invalid_payload("invalid JSON format");
        let response = error.into_response();
        let json: Value = extract_json(response).await;

        assert_eq!(json["error"], "invalid_payload");
        assert_eq!(json["message"], "invalid JSON format");
        assert_eq!(json["code"], 400);
    }

    #[tokio::test]
    async fn test_internal_error_status() {
        let error = ApiError::internal_error("database connection failed");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_internal_error_json() {
        let error = ApiError::internal_error("database connection failed");
        let response = error.into_response();
        let json: Value = extract_json(response).await;

        assert_eq!(json["error"], "internal_error");
        assert_eq!(json["message"], "database connection failed");
        assert_eq!(json["code"], 500);
    }

    #[tokio::test]
    async fn test_ferres_error_conversion() {
        let core_error = FerresError::CollectionNotFound("my-collection".to_string());
        let api_error: ApiError = core_error.into();
        let response = api_error.into_response();
        let status = response.status();
        let json: Value = extract_json(response).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "collection_not_found");
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn test_error_type_methods() {
        let not_found = ApiError::collection_not_found("test");
        assert_eq!(not_found.error_type(), "collection_not_found");
        assert_eq!(not_found.status_code(), StatusCode::NOT_FOUND);

        let invalid_dim = ApiError::invalid_dimension("test");
        assert_eq!(invalid_dim.error_type(), "invalid_dimension");
        assert_eq!(invalid_dim.status_code(), StatusCode::BAD_REQUEST);

        let invalid_payload = ApiError::invalid_payload("test");
        assert_eq!(invalid_payload.error_type(), "invalid_payload");
        assert_eq!(invalid_payload.status_code(), StatusCode::BAD_REQUEST);

        let internal = ApiError::internal_error("test");
        assert_eq!(internal.error_type(), "internal_error");
        assert_eq!(internal.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
