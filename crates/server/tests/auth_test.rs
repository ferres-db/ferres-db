//! # Authentication Integration Tests
//!
//! Testes de integração para autenticação via API Key.

use std::net::SocketAddr;
use tempfile::TempDir;
use tokio::sync::oneshot;

use ferres_db_server::auth;
use ferres_db_server::middleware;
use ferres_db_server::routes;
use ferres_db_server::state::{AppState, ServerConfig};

/// API key usada nos testes de autenticação.
const TEST_API_KEY: &str = "test-key-123";

// ─── Test Helpers ───────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
}

/// Inicia um servidor de teste com autenticação habilitada.
async fn setup_server() -> TestServer {
    // Configura API key para testes
    std::env::set_var("FERRESDB_API_KEYS", TEST_API_KEY);
    auth::init_api_keys();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);

    let temp_dir = tempfile::tempdir().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        storage_path: storage_path.clone(),
        log_level: "error".to_string(),
        api_keys: Some(TEST_API_KEY.to_string()),
        ..Default::default()
    };

    let app_state = AppState::new(config.clone(), None, None, None, None, None).unwrap();

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

    let base_url = format!("http://127.0.0.1:{port}");

    TestServer {
        base_url,
        _temp_dir: temp_dir,
        _shutdown: shutdown_tx,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_requires_api_key() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Sem API key = 401
    let res = client
        .get(format!("{}/api/v1/collections", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // Com API key inválida = 403
    let res = client
        .get(format!("{}/api/v1/collections", server.base_url))
        .header("Authorization", "Bearer wrong-key")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    // Com API key válida = 200
    let res = client
        .get(format!("{}/api/v1/collections", server.base_url))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_health_endpoint_public() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Health não requer API key
    let res = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_metrics_endpoint_public() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Metrics não requer API key
    let res = client
        .get(format!("{}/metrics", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_save_endpoint_protected() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Save sem API key = 401
    let res = client
        .post(format!("{}/api/v1/save", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // Save com API key válida = 200
    let res = client
        .post(format!("{}/api/v1/save", server.base_url))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn test_malformed_auth_header() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Sem prefixo "Bearer " = 401
    let res = client
        .get(format!("{}/api/v1/collections", server.base_url))
        .header("Authorization", TEST_API_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // Prefixo errado = 401
    let res = client
        .get(format!("{}/api/v1/collections", server.base_url))
        .header("Authorization", format!("Basic {TEST_API_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}
