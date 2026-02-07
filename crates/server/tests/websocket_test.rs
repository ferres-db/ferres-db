//! # WebSocket Integration Tests
//!
//! Testes E2E para ingestão em tempo real via WebSocket.

use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tempfile::TempDir;

use ferres_db_server::auth;
use ferres_db_server::middleware;
use ferres_db_server::routes;
use ferres_db_server::state::{AppState, ServerConfig};

/// API key usada nos testes.
const TEST_API_KEY: &str = "test-ws-key-123";

// ─── Test Helpers ───────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    ws_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
}

/// Inicia um servidor de teste com autenticação habilitada.
async fn setup_server() -> TestServer {
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
    };

    let app_state = AppState::new(config.clone(), None, None).unwrap();

    let app = routes::create_router()
        .layer(axum::middleware::from_fn(middleware::request_logger))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(app_state);

    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let _server_handle = tokio::spawn(async move {
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                shutdown_rx.await.ok();
            });
        server.await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let base_url = format!("http://127.0.0.1:{}", port);
    let ws_url = format!("ws://127.0.0.1:{}", port);

    TestServer {
        base_url,
        ws_url,
        _temp_dir: temp_dir,
        _shutdown: shutdown_tx,
    }
}

/// Cria uma coleção via REST para uso nos testes.
async fn create_collection(base_url: &str, name: &str, dimension: usize) {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/collections", base_url))
        .header("Authorization", format!("Bearer {}", TEST_API_KEY))
        .json(&json!({
            "name": name,
            "dimension": dimension,
            "distance": "Cosine"
        }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success() || resp.status().as_u16() == 409,
        "failed to create collection: {}",
        resp.status()
    );
}

/// Faz upsert de pontos via REST.
async fn rest_upsert_points(
    base_url: &str,
    collection: &str,
    points: serde_json::Value,
) -> reqwest::Response {
    let client = reqwest::Client::new();
    client
        .post(format!(
            "{}/api/v1/collections/{}/points",
            base_url, collection
        ))
        .header("Authorization", format!("Bearer {}", TEST_API_KEY))
        .json(&json!({ "points": points }))
        .send()
        .await
        .unwrap()
}

// ─── Tests ──────────────────────────────────────────────────────────────

/// Testa conexão WebSocket com autenticação e upsert de 10 pontos.
#[tokio::test]
async fn test_ws_upsert_10_points() {
    let server = setup_server().await;
    create_collection(&server.base_url, "ws-test-upsert", 3).await;

    let url = format!(
        "{}/api/v1/ws?token={}",
        server.ws_url, TEST_API_KEY
    );

    let (ws_stream, _response) = connect_async(&url).await.expect("failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Gera 10 pontos
    let points: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            json!({
                "id": format!("pt-{}", i),
                "vector": [i as f32 * 0.1, 0.5, 0.3],
                "metadata": {"index": i}
            })
        })
        .collect();

    let msg = json!({
        "type": "upsert",
        "collection": "ws-test-upsert",
        "points": points
    });

    write
        .send(Message::Text(msg.to_string()))
        .await
        .expect("failed to send upsert");

    // Espera ack (ignora pings do heartbeat)
    let ack_json = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(msg)) = read.next().await {
                let text = msg.into_text().unwrap_or_default();
                let json: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "ack" {
                    return json;
                }
            }
        }
    })
    .await
    .expect("timeout waiting for ack");

    assert_eq!(ack_json["type"], "ack");
    assert_eq!(ack_json["upserted"], 10);
    assert_eq!(ack_json["failed"], 0);
    assert!(ack_json["took_ms"].as_u64().is_some());

    write.close().await.ok();
}

/// Testa ping/pong.
#[tokio::test]
async fn test_ws_ping_pong() {
    let server = setup_server().await;

    let url = format!(
        "{}/api/v1/ws?token={}",
        server.ws_url, TEST_API_KEY
    );

    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Envia ping
    let msg = json!({"type": "ping"});
    write
        .send(Message::Text(msg.to_string()))
        .await
        .expect("failed to send ping");

    // Espera pong (ignora server heartbeat pings)
    let pong_json = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(msg)) = read.next().await {
                let text = msg.into_text().unwrap_or_default();
                let json: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "pong" {
                    return json;
                }
            }
        }
    })
    .await
    .expect("timeout waiting for pong");

    assert_eq!(pong_json["type"], "pong");

    write.close().await.ok();
}

/// Testa erro para coleção inexistente.
#[tokio::test]
async fn test_ws_upsert_collection_not_found() {
    let server = setup_server().await;

    let url = format!(
        "{}/api/v1/ws?token={}",
        server.ws_url, TEST_API_KEY
    );

    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
    let (mut write, mut read) = ws_stream.split();

    let msg = json!({
        "type": "upsert",
        "collection": "nonexistent",
        "points": [{"id": "p1", "vector": [0.1, 0.2], "metadata": {}}]
    });

    write
        .send(Message::Text(msg.to_string()))
        .await
        .expect("failed to send");

    // Espera resposta de erro (ignora pings do heartbeat)
    let resp_json = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(msg)) = read.next().await {
                let text = msg.into_text().unwrap_or_default();
                let json: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "error" {
                    return json;
                }
                // Skip pings and other messages
            }
        }
    })
    .await
    .expect("timeout waiting for error");

    assert_eq!(resp_json["type"], "error");
    assert_eq!(resp_json["code"], 404);

    write.close().await.ok();
}

/// Testa subscribe + evento via REST upsert.
#[tokio::test]
async fn test_ws_subscribe_and_receive_event() {
    let server = setup_server().await;
    let coll_name = "ws-test-subscribe";
    create_collection(&server.base_url, coll_name, 3).await;

    let url = format!(
        "{}/api/v1/ws?token={}",
        server.ws_url, TEST_API_KEY
    );

    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Subscribe
    let sub_msg = json!({
        "type": "subscribe",
        "collection": coll_name,
        "events": ["upsert"]
    });
    write
        .send(Message::Text(sub_msg.to_string()))
        .await
        .expect("failed to send subscribe");

    // Espera ack da subscrição
    let sub_ack = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("timeout waiting for sub ack")
        .expect("stream ended")
        .expect("ws error");

    let sub_ack_text = sub_ack.into_text().expect("not text");
    let sub_ack_json: serde_json::Value = serde_json::from_str(&sub_ack_text).unwrap();
    assert_eq!(sub_ack_json["type"], "ack");

    // Dá tempo para a subscription task se estabelecer
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Faz upsert via REST
    let points = json!([
        {"id": "rest-pt-1", "vector": [0.1, 0.2, 0.3], "metadata": {}},
        {"id": "rest-pt-2", "vector": [0.4, 0.5, 0.6], "metadata": {}}
    ]);
    let rest_resp = rest_upsert_points(&server.base_url, coll_name, points).await;
    assert!(rest_resp.status().is_success());

    // Espera evento no WebSocket
    // O servidor pode enviar heartbeat pings, então precisamos filtrar
    let event = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(msg)) = read.next().await {
                let text = msg.into_text().unwrap_or_default();
                let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "event" {
                    return json;
                }
                // Skip pings/pongs/other messages
            }
        }
    })
    .await
    .expect("timeout waiting for event");

    assert_eq!(event["type"], "event");
    assert_eq!(event["collection"], coll_name);
    assert_eq!(event["action"], "upsert");
    let point_ids = event["point_ids"].as_array().unwrap();
    assert!(point_ids.len() >= 2);
    assert!(event["timestamp"].as_u64().is_some());

    write.close().await.ok();
}

/// Testa rejeição sem autenticação.
#[tokio::test]
async fn test_ws_unauthenticated_rejected() {
    let server = setup_server().await;

    let url = format!("{}/api/v1/ws", server.ws_url);

    // Tentativa de conexão sem token — deve falhar
    let result = connect_async(&url).await;
    assert!(result.is_err(), "connection should be rejected without token");
}

/// Testa mensagem inválida.
#[tokio::test]
async fn test_ws_invalid_message() {
    let server = setup_server().await;

    let url = format!(
        "{}/api/v1/ws?token={}",
        server.ws_url, TEST_API_KEY
    );

    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Envia JSON inválido
    write
        .send(Message::Text("not valid json".to_string()))
        .await
        .expect("failed to send");

    // Espera resposta de erro (ignora pings do heartbeat)
    let resp_json = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(msg)) = read.next().await {
                let text = msg.into_text().unwrap_or_default();
                let json: serde_json::Value =
                    serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "error" {
                    return json;
                }
            }
        }
    })
    .await
    .expect("timeout waiting for error");

    assert_eq!(resp_json["type"], "error");
    assert_eq!(resp_json["code"], 400);

    write.close().await.ok();
}
