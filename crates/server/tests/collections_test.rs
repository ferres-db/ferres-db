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

// ─── Explain Search E2E Tests ────────────────────────────────────────────

#[tokio::test]
async fn test_explain_search_endpoint() {
    let server = setup_server().await;

    // 1. Cria coleção
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let response = server
        .client
        .post(&create_url)
        .json(&create_collection_request("explain-test", 3, "euclidean"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);

    // 2. Insere pontos com metadata
    let upsert_url = format!("{}/api/v1/collections/explain-test/points", server.base_url);
    let upsert_body = serde_json::json!({
        "points": [
            {"id": "p1", "vector": [1.0, 0.0, 0.0], "metadata": {"category": "tech", "price": 50}},
            {"id": "p2", "vector": [0.0, 1.0, 0.0], "metadata": {"category": "science", "price": 200}},
            {"id": "p3", "vector": [0.9, 0.1, 0.0], "metadata": {"category": "tech", "price": 30}}
        ]
    });
    let response = server
        .client
        .post(&upsert_url)
        .json(&upsert_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // 3. Faz POST /search/explain sem filtro
    let explain_url = format!(
        "{}/api/v1/collections/explain-test/search/explain",
        server.base_url
    );
    let explain_body = serde_json::json!({
        "vector": [1.0, 0.0, 0.0],
        "limit": 5
    });
    let response = server
        .client
        .post(&explain_url)
        .json(&explain_body)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(status, reqwest::StatusCode::OK, "body: {:?}", body);

    // Verifica schema da resposta
    assert!(body["query_vector_norm"].is_number());
    assert!(body["distance_metric"].is_string());
    assert!(body["candidates_scanned"].is_number());
    assert!(body["candidates_after_filter"].is_number());
    assert!(body["results"].is_array());
    assert!(body["index_stats"].is_object());

    // Verifica resultados
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty());

    // Verifica schema de cada resultado
    for result in results {
        assert!(result["id"].is_string());
        assert!(result["score"].is_number());
        assert!(result["distance_metric"].is_string());
        assert!(result["raw_distance"].is_number());
        assert!(result["score_breakdown"].is_object());
        assert!(result["score_breakdown"]["vector_score"].is_number());
        assert!(result["rank_before_filter"].is_number());
    }

    // Verifica index_stats
    assert!(body["index_stats"]["total_points"].is_number());
    assert!(body["index_stats"]["hnsw_layers"].is_number());
    assert!(body["index_stats"]["ef_search_used"].is_number());
    assert!(body["index_stats"]["tombstones_skipped"].is_number());

    // 4. Faz POST /search/explain COM filtro
    let explain_body_filtered = serde_json::json!({
        "vector": [1.0, 0.0, 0.0],
        "limit": 5,
        "filter": {
            "category": "tech",
            "price": { "$lte": 100 }
        }
    });
    let response = server
        .client
        .post(&explain_url)
        .json(&explain_body_filtered)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(status, reqwest::StatusCode::OK, "body: {:?}", body);

    // Com filtro, cada resultado deve ter filter_evaluation
    let results = body["results"].as_array().unwrap();
    for result in results {
        assert!(
            result["filter_evaluation"].is_object(),
            "filter_evaluation deve estar presente quando filtro é aplicado"
        );
        let eval = &result["filter_evaluation"];
        assert!(eval["conditions"].is_array());
        assert!(eval["passed"].is_boolean());
    }

    // candidates_after_filter deve ser <= candidates_scanned
    let scanned = body["candidates_scanned"].as_u64().unwrap();
    let after_filter = body["candidates_after_filter"].as_u64().unwrap();
    assert!(after_filter <= scanned);
}

// ─── Estimate Search Cost E2E Tests ──────────────────────────────────────

#[tokio::test]
async fn test_estimate_search_endpoint() {
    let server = setup_server().await;

    // 1. Cria coleção
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let response = server
        .client
        .post(&create_url)
        .json(&create_collection_request("estimate-test", 3, "euclidean"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);

    // 2. Insere pontos
    let upsert_url = format!(
        "{}/api/v1/collections/estimate-test/points",
        server.base_url
    );
    let upsert_body = serde_json::json!({
        "points": [
            {"id": "p1", "vector": [1.0, 0.0, 0.0], "metadata": {"category": "tech"}},
            {"id": "p2", "vector": [0.0, 1.0, 0.0], "metadata": {"category": "science"}},
            {"id": "p3", "vector": [0.9, 0.1, 0.0], "metadata": {"category": "tech"}}
        ]
    });
    let response = server
        .client
        .post(&upsert_url)
        .json(&upsert_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // 3. Estima custo sem filtro
    let estimate_url = format!(
        "{}/api/v1/collections/estimate-test/search/estimate",
        server.base_url
    );
    let estimate_body = serde_json::json!({
        "limit": 5
    });
    let response = server
        .client
        .post(&estimate_url)
        .json(&estimate_body)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body: {:?}", body);

    // Verifica schema
    assert!(body["estimated_ms"].is_number());
    assert!(body["confidence_range"].is_array());
    assert!(body["estimated_memory_bytes"].is_number());
    assert!(body["estimated_nodes_visited"].is_number());
    assert!(body["is_expensive"].is_boolean());
    assert!(body["recommendations"].is_array());
    assert!(body["breakdown"].is_object());
    assert!(body["breakdown"]["index_scan_cost"].is_number());
    assert!(body["breakdown"]["filter_cost"].is_number());
    assert!(body["breakdown"]["hydration_cost"].is_number());
    assert!(body["breakdown"]["network_overhead"].is_number());

    // historical_latency não deve estar presente (não solicitado)
    assert!(body["historical_latency"].is_null());

    // 4. Estima custo com filtro e include_history
    let estimate_body_with_filter = serde_json::json!({
        "limit": 10,
        "filter": { "category": "tech" },
        "include_history": true
    });
    let response = server
        .client
        .post(&estimate_url)
        .json(&estimate_body_with_filter)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body: {:?}", body);

    // Com filtro, filter_cost deve ser > 0
    assert!(
        body["breakdown"]["filter_cost"].as_f64().unwrap() > 0.0,
        "filter_cost should be > 0 when filter is present"
    );

    // Com include_history, historical_latency deve estar presente
    assert!(body["historical_latency"].is_object());
    assert!(body["historical_latency"]["p50_ms"].is_number());
    assert!(body["historical_latency"]["p95_ms"].is_number());
    assert!(body["historical_latency"]["p99_ms"].is_number());
    assert!(body["historical_latency"]["avg_ms"].is_number());
    assert!(body["historical_latency"]["total_queries"].is_number());
}

#[tokio::test]
async fn test_estimate_search_collection_not_found() {
    let server = setup_server().await;

    let estimate_url = format!(
        "{}/api/v1/collections/nonexistent/search/estimate",
        server.base_url
    );
    let estimate_body = serde_json::json!({ "limit": 5 });
    let response = server
        .client
        .post(&estimate_url)
        .json(&estimate_body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_budget_ms_search_rejected() {
    let server = setup_server().await;

    // 1. Cria coleção
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let response = server
        .client
        .post(&create_url)
        .json(&create_collection_request("budget-test", 3, "euclidean"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);

    // 2. Insere pontos
    let upsert_url = format!(
        "{}/api/v1/collections/budget-test/points",
        server.base_url
    );
    let upsert_body = serde_json::json!({
        "points": [
            {"id": "p1", "vector": [1.0, 0.0, 0.0], "metadata": {}},
            {"id": "p2", "vector": [0.0, 1.0, 0.0], "metadata": {}}
        ]
    });
    let response = server
        .client
        .post(&upsert_url)
        .json(&upsert_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // 3. Busca com budget_ms muito baixo (0ms) — deve ser rejeitada
    let search_url = format!(
        "{}/api/v1/collections/budget-test/search",
        server.base_url
    );
    let search_body = serde_json::json!({
        "vector": [1.0, 0.0, 0.0],
        "limit": 5,
        "budget_ms": 0
    });
    let response = server
        .client
        .post(&search_url)
        .json(&search_body)
        .send()
        .await
        .unwrap();

    // Deve retornar 422 com estimate no body
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "budget_ms=0 should always be rejected"
    );

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "budget_exceeded");
    assert!(body["estimate"].is_object());
    assert!(body["estimate"]["estimated_ms"].is_number());
    assert!(body["estimate"]["breakdown"].is_object());
}

#[tokio::test]
async fn test_budget_ms_search_accepted() {
    let server = setup_server().await;

    // 1. Cria coleção
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let response = server
        .client
        .post(&create_url)
        .json(&create_collection_request("budget-accept-test", 3, "euclidean"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);

    // 2. Insere pontos
    let upsert_url = format!(
        "{}/api/v1/collections/budget-accept-test/points",
        server.base_url
    );
    let upsert_body = serde_json::json!({
        "points": [
            {"id": "p1", "vector": [1.0, 0.0, 0.0], "metadata": {}}
        ]
    });
    let response = server
        .client
        .post(&upsert_url)
        .json(&upsert_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // 3. Busca com budget_ms muito alto (10 segundos) — deve funcionar
    let search_url = format!(
        "{}/api/v1/collections/budget-accept-test/search",
        server.base_url
    );
    let search_body = serde_json::json!({
        "vector": [1.0, 0.0, 0.0],
        "limit": 1,
        "budget_ms": 10000
    });
    let response = server
        .client
        .post(&search_url)
        .json(&search_body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["results"].is_array());
}
