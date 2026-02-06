//! # Collection Integration Tests
//!
//! Testes de integração para os endpoints de gerenciamento de coleções.
//!
//! Para **testes de propriedade** (quickcheck), **concorrência** e **carga**,
//! ver `property_tests.rs`.

use std::net::SocketAddr;
use tokio::sync::oneshot;
use tempfile::TempDir;

use ferres_db_server::auth;
use ferres_db_server::routes;
use ferres_db_server::state::{AppState, ServerConfig};
use ferres_db_server::middleware;

/// API key usada em todos os testes de integração.
const TEST_API_KEY: &str = "test-key-collections";

// ─── Test Helpers ───────────────────────────────────────────────────────

/// Configuração do servidor de teste com porta aleatória.
struct TestServer {
    client: reqwest::Client,
    base_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
}

/// Inicia um servidor de teste em uma porta aleatória.
///
/// Retorna um cliente HTTP (com API key nos headers) e um callback de cleanup.
async fn setup_server() -> TestServer {
    // Configura API key para testes
    std::env::set_var("FERRESDB_API_KEYS", TEST_API_KEY);
    auth::init_api_keys();

    // Encontra uma porta livre
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);

    // Cria diretório temporário para storage
    let temp_dir = tempfile::tempdir().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    // Configuração do servidor de teste
    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        storage_path: storage_path.clone(),
        log_level: "error".to_string(), // Reduz logs durante testes
        api_keys: Some(TEST_API_KEY.to_string()),
    };

    // Inicializa AppState
    let app_state = AppState::new(config.clone(), None, None).unwrap();

    // Cria o router com middleware
    let app = routes::create_router()
        .layer(axum::middleware::from_fn(middleware::request_logger))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(app_state);

    // Cria o listener
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    // Canal para shutdown
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // Inicia servidor em background
    let _server_handle = tokio::spawn(async move {
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                shutdown_rx.await.ok();
            });
        server.await.unwrap();
    });

    // Aguarda um pouco para o servidor iniciar
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let base_url = format!("http://127.0.0.1:{}", port);

    // Cliente com API key default em todos os requests
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {}", TEST_API_KEY).parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();

    TestServer {
        client,
        base_url,
        _temp_dir: temp_dir,
        _shutdown: shutdown_tx,
    }
}

// ─── Test Fixtures ───────────────────────────────────────────────────────

fn create_collection_request(name: &str, dimension: usize, distance: &str) -> serde_json::Value {
    // DistanceMetric é serializado como: "Cosine", "DotProduct", "Euclidean"
    let distance_upper = match distance.to_lowercase().as_str() {
        "cosine" => "Cosine",
        "dot" | "dotproduct" => "DotProduct",
        "euclidean" => "Euclidean",
        _ => distance, // Usa como está se não reconhecer
    };
    
    serde_json::json!({
        "name": name,
        "dimension": dimension,
        "distance": distance_upper
    })
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_collection_success() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections", server.base_url);

    let response = server
        .client
        .post(&url)
        .json(&create_collection_request("test-collection", 128, "cosine"))
        .send()
        .await
        .unwrap();

    let status = response.status();
    if status != reqwest::StatusCode::CREATED {
        let error_body = response.text().await.unwrap();
        panic!("Expected 201 CREATED, got {}: {}", status, error_body);
    }

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "test-collection");
    assert_eq!(body["dimension"], 128);
    assert_eq!(body["distance"], "Cosine");
    assert!(body["created_at"].is_number());
}

#[tokio::test]
async fn test_create_collection_duplicate() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections", server.base_url);

    // Cria primeira coleção
    let response1 = server
        .client
        .post(&url)
        .json(&create_collection_request("duplicate-test", 64, "euclidean"))
        .send()
        .await
        .unwrap();
    assert_eq!(response1.status(), reqwest::StatusCode::CREATED);

    // Tenta criar novamente com mesmo nome
    let response2 = server
        .client
        .post(&url)
        .json(&create_collection_request("duplicate-test", 64, "euclidean"))
        .send()
        .await
        .unwrap();

    assert_eq!(response2.status(), reqwest::StatusCode::CONFLICT);

    let body: serde_json::Value = response2.json().await.unwrap();
    assert_eq!(body["error"], "collection_already_exists");
    assert!(body["message"].as_str().unwrap().contains("duplicate-test"));
}

#[tokio::test]
async fn test_create_collection_invalid_name() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections", server.base_url);

    // Testa nome vazio
    let response1 = server
        .client
        .post(&url)
        .json(&create_collection_request("", 128, "cosine"))
        .send()
        .await
        .unwrap();
    assert_eq!(response1.status(), reqwest::StatusCode::BAD_REQUEST);

    // Testa nome com caracteres especiais
    let response2 = server
        .client
        .post(&url)
        .json(&create_collection_request("invalid@name", 128, "cosine"))
        .send()
        .await
        .unwrap();
    assert_eq!(response2.status(), reqwest::StatusCode::BAD_REQUEST);

    let body: serde_json::Value = response2.json().await.unwrap();
    assert_eq!(body["error"], "invalid_payload");
}

#[tokio::test]
async fn test_create_collection_invalid_dimension() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections", server.base_url);

    // Testa dimensão muito pequena
    let response1 = server
        .client
        .post(&url)
        .json(&create_collection_request("test", 0, "cosine"))
        .send()
        .await
        .unwrap();
    assert_eq!(response1.status(), reqwest::StatusCode::BAD_REQUEST);

    // Testa dimensão muito grande
    let response2 = server
        .client
        .post(&url)
        .json(&create_collection_request("test", 5000, "cosine"))
        .send()
        .await
        .unwrap();
    assert_eq!(response2.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_collection_not_found() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections/nonexistent", server.base_url);

    let response = server
        .client
        .get(&url)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "collection_not_found");
    assert!(body["message"].as_str().unwrap().contains("nonexistent"));
}

#[tokio::test]
async fn test_list_collections_empty() {
    let server = setup_server().await;
    let url = format!("{}/api/v1/collections", server.base_url);

    let response = server
        .client
        .get(&url)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["collections"].is_array());
    assert_eq!(body["collections"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_collections_with_data() {
    let server = setup_server().await;
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let list_url = format!("{}/api/v1/collections", server.base_url);

    // Cria algumas coleções
    server
        .client
        .post(&create_url)
        .json(&create_collection_request("collection-1", 64, "cosine"))
        .send()
        .await
        .unwrap();

    server
        .client
        .post(&create_url)
        .json(&create_collection_request("collection-2", 128, "euclidean"))
        .send()
        .await
        .unwrap();

    // Lista todas as coleções
    let response = server
        .client
        .get(&list_url)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 2);

    // Verifica que ambas as coleções estão na lista
    let names: Vec<&str> = collections
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"collection-1"));
    assert!(names.contains(&"collection-2"));
}

#[tokio::test]
async fn test_get_collection_success() {
    let server = setup_server().await;
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let get_url = format!("{}/api/v1/collections/test-collection", server.base_url);

    // Cria uma coleção
    server
        .client
        .post(&create_url)
        .json(&create_collection_request("test-collection", 256, "dot"))
        .send()
        .await
        .unwrap();

    // Busca a coleção
    let response = server
        .client
        .get(&get_url)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "test-collection");
    assert_eq!(body["dimension"], 256);
    assert_eq!(body["num_points"], 0);
    assert!(body["last_updated"].is_number());
    assert!(body["stats"]["index_size_bytes"].is_number());
}

#[tokio::test]
async fn test_delete_collection() {
    let server = setup_server().await;
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let delete_url = format!("{}/api/v1/collections/test-delete", server.base_url);
    let get_url = format!("{}/api/v1/collections/test-delete", server.base_url);

    // Cria uma coleção
    server
        .client
        .post(&create_url)
        .json(&create_collection_request("test-delete", 64, "cosine"))
        .send()
        .await
        .unwrap();

    // Verifica que a coleção existe
    let response1 = server
        .client
        .get(&get_url)
        .send()
        .await
        .unwrap();
    assert_eq!(response1.status(), reqwest::StatusCode::OK);

    // Deleta a coleção
    let response2 = server
        .client
        .delete(&delete_url)
        .send()
        .await
        .unwrap();
    assert_eq!(response2.status(), reqwest::StatusCode::NO_CONTENT);

    // Verifica que a coleção não existe mais
    let response3 = server
        .client
        .get(&get_url)
        .send()
        .await
        .unwrap();
    assert_eq!(response3.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_collection_not_found() {
    let server = setup_server().await;
    let delete_url = format!("{}/api/v1/collections/nonexistent", server.base_url);

    let response = server
        .client
        .delete(&delete_url)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "collection_not_found");
}

