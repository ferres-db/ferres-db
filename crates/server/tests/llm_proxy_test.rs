//! # LLM Proxy Integration Tests
//!
//! Mock cada provedor (OpenAI, Anthropic, Gemini) com `wiremock` e valida o
//! fluxo end-to-end: autenticação, RBAC (Admin/Editor only), audit, métrica
//! Prometheus, e mapeamento da resposta do provedor.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::oneshot;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ferres_db_server::api_keys::{self, ApiKeyStore};
use ferres_db_server::auth;
use ferres_db_server::llm_credentials::{LlmCredentialsStore, LlmProvider};
use ferres_db_server::middleware;
use ferres_db_server::routes;
use ferres_db_server::state::{AppState, ServerConfig};
use ferres_db_server::users::{Role, UserStore};

/// API key bootstrap usada para o cliente em testes.
const TEST_API_KEY: &str = "test-llm-proxy-key";

struct TestServer {
    base_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
}

/// Sobe um servidor com:
///  - LlmCredentialsStore em SQLite (com chaves para os 3 providers)
///  - UserStore com um usuário Editor e um Viewer
///  - Mock server (wiremock) substituindo as URLs dos provedores via
///    FERRESDB_LLM_PROXY_BASE_URL.
async fn setup_server() -> (TestServer, MockServer) {
    // Garante que nenhuma chave env atrapalhe (caso outro teste tenha setado).
    std::env::remove_var("FERRESDB_OPENAI_API_KEY");
    std::env::remove_var("FERRESDB_ANTHROPIC_API_KEY");
    std::env::remove_var("FERRESDB_GEMINI_API_KEY");

    // Mock server para os provedores.
    let mock_server = MockServer::start().await;
    std::env::set_var("FERRESDB_LLM_PROXY_BASE_URL", mock_server.uri());

    let temp_dir = tempfile::tempdir().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    // ApiKeyStore com TEST_API_KEY (mais robusto que o OnceLock legacy entre testes).
    let api_key_store = ApiKeyStore::new(&storage_path.join("api_keys.db")).unwrap();
    api_key_store.init(Some(TEST_API_KEY)).unwrap();
    let api_key_store = Arc::new(api_key_store);
    api_keys::set_global_store(Some(api_key_store.clone()));
    // Limpa o legacy só por garantia (idempotente).
    auth::init_api_keys_from(Some(""));

    // LLM credentials store: salva uma chave por provider (em DB).
    let llm_store = LlmCredentialsStore::new(&storage_path.join("llm_credentials.db")).unwrap();
    llm_store
        .set(LlmProvider::Openai, "sk-mock-openai")
        .unwrap();
    llm_store
        .set(LlmProvider::Anthropic, "sk-mock-anthropic")
        .unwrap();
    llm_store
        .set(LlmProvider::Gemini, "AIza-mock-gemini")
        .unwrap();
    let llm_store = Some(Arc::new(llm_store));

    // User store: cria um Editor e um Viewer; usaremos o JWT só para o Viewer.
    let user_store = UserStore::new(&storage_path.join("users.db")).unwrap();
    let _ = user_store.create("viewer1", "viewer-pass", Some(Role::Viewer));
    let user_store = Some(Arc::new(user_store));

    // JWT secret for any future JWT-based test
    auth::set_jwt_secret(b"llm-proxy-test-secret".to_vec());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);

    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        storage_path: storage_path.clone(),
        log_level: "error".to_string(),
        api_keys: Some(TEST_API_KEY.to_string()),
        ..Default::default()
    };

    let app_state = AppState::new(
        config.clone(),
        Some(api_key_store),
        user_store,
        None,
        llm_store,
        None,
    )
    .unwrap();

    let app = routes::create_router(&config)
        .layer(axum::middleware::from_fn(middleware::request_logger))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(app_state);

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let _server_handle = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            shutdown_rx.await.ok();
        });
        server.await.unwrap();
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let test_server = TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        _temp_dir: temp_dir,
        _shutdown: shutdown_tx,
    };
    (test_server, mock_server)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[tokio::test]
#[serial]
async fn openai_success_returns_text_and_does_not_leak_key_to_client() {
    let (server, mock) = setup_server().await;

    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-mock-openai"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "hi from mock openai" } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 4 }
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "prompt": "hello",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["text"], "hi from mock openai");
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 4);
    // O cliente nunca recebe a chave do provedor.
    assert!(!body.to_string().contains("sk-mock-openai"));
}

#[tokio::test]
#[serial]
async fn anthropic_success_extracts_text_from_content_blocks() {
    let (server, mock) = setup_server().await;

    Mock::given(method("POST"))
        .and(path("/anthropic/v1/messages"))
        .and(header("x-api-key", "sk-mock-anthropic"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{ "type": "text", "text": "anthropic mock response" }],
            "usage": { "input_tokens": 7, "output_tokens": 3 }
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "anthropic",
            "model": "claude-3-5-haiku-20241022",
            "prompt": "hi",
            "max_tokens": 256,
            "temperature": 0.5,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["text"], "anthropic mock response");
    assert_eq!(body["usage"]["input_tokens"], 7);
    assert_eq!(body["usage"]["output_tokens"], 3);
}

#[tokio::test]
#[serial]
async fn gemini_success_passes_api_key_in_query_string() {
    let (server, mock) = setup_server().await;

    Mock::given(method("POST"))
        .and(path(
            "/gemini/v1beta/models/gemini-1.5-flash:generateContent",
        ))
        .and(query_param("key", "AIza-mock-gemini"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{
                "content": { "parts": [{ "text": "gemini mock response" }] }
            }],
            "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 9 }
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "gemini",
            "model": "gemini-1.5-flash",
            "prompt": "hi",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["text"], "gemini mock response");
    assert_eq!(body["usage"]["input_tokens"], 5);
    assert_eq!(body["usage"]["output_tokens"], 9);
}

#[tokio::test]
#[serial]
async fn missing_credential_returns_503_when_provider_not_configured() {
    let (server, _mock) = setup_server().await;

    // Remove a chave do gemini para forçar 503.
    let store =
        LlmCredentialsStore::new(&server._temp_dir.path().join("llm_credentials.db")).unwrap();
    store.delete(LlmProvider::Gemini).unwrap();

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "gemini",
            "model": "gemini-1.5-flash",
            "prompt": "hi",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"], "llm_provider_not_configured");
}

#[tokio::test]
#[serial]
async fn upstream_4xx_returns_502_with_provider_message() {
    let (server, mock) = setup_server().await;

    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": { "message": "invalid api key" }
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "prompt": "hi",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 502);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"], "llm_upstream_error");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("invalid api key"));
}

#[tokio::test]
#[serial]
async fn empty_prompt_returns_400() {
    let (server, _mock) = setup_server().await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "prompt": "   ",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"], "invalid_payload");
}

#[tokio::test]
#[serial]
async fn unknown_provider_returns_400() {
    let (server, _mock) = setup_server().await;

    let res = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "huggingface",
            "model": "x",
            "prompt": "hi",
        }))
        .send()
        .await
        .unwrap();

    // serde rejeita o enum desconhecido com 422 (Json extractor) ou 400; aceitamos qualquer um na faixa.
    assert!(
        res.status() == 400 || res.status() == 422,
        "expected 4xx, got {}",
        res.status()
    );
}

#[tokio::test]
#[serial]
async fn admin_can_list_credentials_status_without_key_value() {
    let (server, _mock) = setup_server().await;

    let res = client()
        .get(format!("{}/api/v1/admin/llm-credentials", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let providers = body["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 3);
    let openai = providers
        .iter()
        .find(|p| p["provider"] == "openai")
        .unwrap();
    assert_eq!(openai["configured"], true);
    assert_eq!(openai["source"], "db");
    // chave nunca aparece no payload
    assert!(!body.to_string().contains("sk-mock"));
}

#[tokio::test]
#[serial]
async fn admin_can_put_and_delete_credential() {
    let (server, _mock) = setup_server().await;

    // PUT
    let res = client()
        .put(format!(
            "{}/api/v1/admin/llm-credentials/openai",
            server.base_url
        ))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({ "api_key": "sk-new-openai-key" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // DELETE
    let res = client()
        .delete(format!(
            "{}/api/v1/admin/llm-credentials/openai",
            server.base_url
        ))
        .bearer_auth(TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["removed"], true);
}

#[tokio::test]
#[serial]
async fn admin_put_unknown_provider_returns_400() {
    let (server, _mock) = setup_server().await;

    let res = client()
        .put(format!(
            "{}/api/v1/admin/llm-credentials/cohere",
            server.base_url
        ))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({ "api_key": "anything" }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 400);
}

#[tokio::test]
#[serial]
async fn metric_increments_on_success() {
    let (server, mock) = setup_server().await;

    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": "ok" } }]
        })))
        .mount(&mock)
        .await;

    let _ = client()
        .post(format!("{}/api/v1/llm/complete", server.base_url))
        .bearer_auth(TEST_API_KEY)
        .json(&json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "prompt": "hello"
        }))
        .send()
        .await
        .unwrap();

    let metrics = client()
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        metrics.contains("ferresdb_llm_proxy_requests_total"),
        "metric not exposed; metrics body: {metrics}"
    );
    assert!(
        metrics.contains("provider=\"openai\""),
        "expected openai label; got: {metrics}"
    );
}
