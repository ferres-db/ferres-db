//! # LLM Proxy — POST /api/v1/llm/complete
//!
//! Faz proxy server-side para OpenAI, Anthropic e Gemini para evitar enviar
//! API keys do navegador. As credenciais vêm de env var (FERRESDB_*_API_KEY)
//! ou da tabela SQLite `llm_credentials` (configurada por Admin no dashboard).
//!
//! Requer role Admin OU Editor (a UI Query Tester é editor-restricted).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, warn};

use crate::audit::{self, AuditResult};
use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::llm_credentials::LlmProvider;
use crate::metrics::LLM_PROXY_REQUESTS_TOTAL;
use crate::state::AppState;

/// Request body de POST /api/v1/llm/complete.
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub provider: LlmProvider,
    pub model: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Tokens consumidos pela requisição (quando o provedor reporta).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Response body de POST /api/v1/llm/complete.
#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

const DEFAULT_MAX_TOKENS: u32 = 1024;
const DEFAULT_TEMPERATURE: f32 = 0.7;
const REQUEST_TIMEOUT_SECS: u64 = 60;
const MAX_PROMPT_BYTES: usize = 200_000; // ~200 KB hard cap (não é o limite real do provedor)

/// POST /api/v1/llm/complete
pub async fn complete(
    AuthenticatedUser(user): AuthenticatedUser,
    State(app_state): State<AppState>,
    Json(req): Json<CompleteRequest>,
) -> Response {
    let provider_label = req.provider.as_str();

    // Authorization: Admin ou Editor
    use crate::users::Role;
    if user.role < Role::Editor {
        LLM_PROXY_REQUESTS_TOTAL
            .with_label_values(&[provider_label, "auth_error"])
            .inc();
        return ApiError::forbidden("Admin or Editor role required for LLM proxy").into_response();
    }

    // Validações de payload
    if req.prompt.trim().is_empty() {
        LLM_PROXY_REQUESTS_TOTAL
            .with_label_values(&[provider_label, "error"])
            .inc();
        return ApiError::invalid_payload("prompt cannot be empty").into_response();
    }
    if req.prompt.len() > MAX_PROMPT_BYTES {
        LLM_PROXY_REQUESTS_TOTAL
            .with_label_values(&[provider_label, "error"])
            .inc();
        return ApiError::invalid_payload(format!(
            "prompt exceeds {} byte limit",
            MAX_PROMPT_BYTES
        ))
        .into_response();
    }
    if req.model.trim().is_empty() {
        LLM_PROXY_REQUESTS_TOTAL
            .with_label_values(&[provider_label, "error"])
            .inc();
        return ApiError::invalid_payload("model is required").into_response();
    }

    // Resolve credencial: env primeiro, depois DB.
    let (api_key, key_source) = match resolve_api_key(&app_state, req.provider) {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            LLM_PROXY_REQUESTS_TOTAL
                .with_label_values(&[provider_label, "auth_error"])
                .inc();
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "llm_provider_not_configured",
                    "message": format!(
                        "Provider '{}' has no API key configured. Set {} or configure via Admin → Settings → LLM Credentials.",
                        provider_label,
                        req.provider.env_var()
                    ),
                    "code": 503,
                })),
            )
                .into_response();
        }
        Err(e) => {
            LLM_PROXY_REQUESTS_TOTAL
                .with_label_values(&[provider_label, "error"])
                .inc();
            return ApiError::internal_error(format!("failed to resolve LLM credential: {e}"))
                .into_response();
        }
    };

    let max_tokens = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    let temperature = req.temperature.unwrap_or(DEFAULT_TEMPERATURE);
    let prompt_len = req.prompt.len();
    let model_for_log = req.model.clone();

    let result = match req.provider {
        LlmProvider::Openai => {
            call_openai(&api_key, &req.model, &req.prompt, max_tokens, temperature).await
        }
        LlmProvider::Anthropic => {
            call_anthropic(&api_key, &req.model, &req.prompt, max_tokens, temperature).await
        }
        LlmProvider::Gemini => {
            call_gemini(&api_key, &req.model, &req.prompt, max_tokens, temperature).await
        }
    };

    let (response, audit_result, status_label) = match result {
        Ok(ok) => (
            (
                StatusCode::OK,
                Json(serde_json::to_value(&ok).unwrap_or(json!({}))),
            )
                .into_response(),
            AuditResult::Success,
            "ok",
        ),
        Err(ProxyError::Upstream { status, message }) => {
            warn!(
                provider = provider_label,
                status,
                message = %message,
                "LLM provider returned error"
            );
            let label = if status == 401 || status == 403 {
                "auth_error"
            } else {
                "upstream_error"
            };
            (
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "llm_upstream_error",
                        "message": format!("{} returned {}: {}", provider_label, status, message),
                        "code": 502,
                    })),
                )
                    .into_response(),
                AuditResult::Error,
                label,
            )
        }
        Err(ProxyError::Network(msg)) => {
            warn!(provider = provider_label, error = %msg, "LLM proxy network error");
            (
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "llm_network_error",
                        "message": msg,
                        "code": 502,
                    })),
                )
                    .into_response(),
                AuditResult::Error,
                "upstream_error",
            )
        }
        Err(ProxyError::Timeout) => {
            warn!(provider = provider_label, "LLM proxy timeout");
            (
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(json!({
                        "error": "llm_timeout",
                        "message": format!("LLM provider '{}' timed out", provider_label),
                        "code": 504,
                    })),
                )
                    .into_response(),
                AuditResult::Error,
                "timeout",
            )
        }
        Err(ProxyError::ResponseParse(msg)) => {
            warn!(provider = provider_label, error = %msg, "LLM response parse error");
            (
                ApiError::internal_error(format!(
                    "failed to parse {provider_label} response: {msg}"
                ))
                .into_response(),
                AuditResult::Error,
                "error",
            )
        }
    };

    LLM_PROXY_REQUESTS_TOTAL
        .with_label_values(&[provider_label, status_label])
        .inc();

    // Audit: nunca grava o conteúdo do prompt; apenas tamanho + modelo.
    let audit_details = json!({
        "provider": provider_label,
        "model": model_for_log,
        "prompt_bytes": prompt_len,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "key_source": key_source,
    });
    let entry = audit::audit_entry(
        &user.username,
        "llm_complete",
        &format!("provider:{}", provider_label),
        audit_details,
        audit_result,
        None,
        None,
    );
    app_state.audit_logger.log(&entry);

    response
}

fn resolve_api_key(
    app_state: &AppState,
    provider: LlmProvider,
) -> Result<Option<(String, &'static str)>, crate::llm_credentials::LlmCredentialsError> {
    if let Some(store) = &app_state.llm_credentials_store {
        store.resolve(provider)
    } else {
        // Sem store → tenta apenas env var
        if let Ok(v) = std::env::var(provider.env_var()) {
            if !v.trim().is_empty() {
                return Ok(Some((v, "env")));
            }
        }
        Ok(None)
    }
}

// ─── Internal proxy errors ───────────────────────────────────────────────

#[derive(Debug)]
enum ProxyError {
    /// Provider replied with a non-2xx status.
    Upstream { status: u16, message: String },
    /// reqwest connect/transport error.
    Network(String),
    /// Request timed out.
    Timeout,
    /// Provider replied 2xx but body did not match expected schema.
    ResponseParse(String),
}

impl From<reqwest::Error> for ProxyError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ProxyError::Timeout
        } else {
            ProxyError::Network(e.to_string())
        }
    }
}

fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(concat!("ferresdb-server/", env!("CARGO_PKG_VERSION")))
        .build()
}

async fn read_provider_error(resp: reqwest::Response) -> ProxyError {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    // Tenta extrair {error: {message: "..."}} do payload, senão usa body bruto truncado.
    let message = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(String::from)
            .unwrap_or_else(|| truncate_err(&body)),
        Err(_) => truncate_err(&body),
    };
    ProxyError::Upstream { status, message }
}

fn truncate_err(s: &str) -> String {
    const MAX: usize = 500;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

// ─── OpenAI ──────────────────────────────────────────────────────────────

async fn call_openai(
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: u32,
    temperature: f32,
) -> Result<CompleteResponse, ProxyError> {
    let url = openai_url();
    debug!(url, "calling openai");
    let client = build_client()?;
    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(read_provider_error(resp).await);
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProxyError::ResponseParse(e.to_string()))?;
    let text = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        output_tokens: u
            .get("completion_tokens")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32,
    });
    Ok(CompleteResponse { text, usage })
}

fn openai_url() -> String {
    if let Ok(base) = std::env::var(LLM_PROXY_BASE_URL_ENV) {
        format!("{}/openai/v1/chat/completions", base.trim_end_matches('/'))
    } else {
        "https://api.openai.com/v1/chat/completions".to_string()
    }
}

// ─── Anthropic ───────────────────────────────────────────────────────────

async fn call_anthropic(
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: u32,
    temperature: f32,
) -> Result<CompleteResponse, ProxyError> {
    let url = anthropic_url();
    debug!(url, "calling anthropic");
    let client = build_client()?;
    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let resp = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(read_provider_error(resp).await);
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProxyError::ResponseParse(e.to_string()))?;
    // content é um array de blocks; pega o texto do primeiro bloco type="text".
    let text = v
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                .next()
        })
        .unwrap_or("")
        .to_string();
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        output_tokens: u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
    });
    Ok(CompleteResponse { text, usage })
}

fn anthropic_url() -> String {
    if let Ok(base) = std::env::var(LLM_PROXY_BASE_URL_ENV) {
        format!("{}/anthropic/v1/messages", base.trim_end_matches('/'))
    } else {
        "https://api.anthropic.com/v1/messages".to_string()
    }
}

// ─── Gemini ──────────────────────────────────────────────────────────────

async fn call_gemini(
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: u32,
    temperature: f32,
) -> Result<CompleteResponse, ProxyError> {
    let url = gemini_url(model, api_key);
    debug!(url = url.replace(api_key, "***"), "calling gemini");
    let client = build_client()?;
    let body = json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": {
            "maxOutputTokens": max_tokens,
            "temperature": temperature,
        },
    });
    let resp = client.post(url).json(&body).send().await?;
    if !resp.status().is_success() {
        return Err(read_provider_error(resp).await);
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ProxyError::ResponseParse(e.to_string()))?;
    let text = v
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    // Gemini reports usageMetadata
    let usage = v.get("usageMetadata").map(|u| Usage {
        input_tokens: u
            .get("promptTokenCount")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32,
        output_tokens: u
            .get("candidatesTokenCount")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32,
    });
    Ok(CompleteResponse { text, usage })
}

fn gemini_url(model: &str, api_key: &str) -> String {
    if let Ok(base) = std::env::var(LLM_PROXY_BASE_URL_ENV) {
        format!(
            "{}/gemini/v1beta/models/{model}:generateContent?key={api_key}",
            base.trim_end_matches('/')
        )
    } else {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
        )
    }
}

/// Env var that overrides the LLM provider base URL. Used by integration tests
/// to redirect requests to a mock server (e.g. wiremock). Production deployments
/// should leave this unset.
const LLM_PROXY_BASE_URL_ENV: &str = "FERRESDB_LLM_PROXY_BASE_URL";
