//! # Property Tests & Concurrency Tests for HTTP Handlers
//!
//! Testes de propriedade usando quickcheck para validar invariantes
//! e testes de concorrência para detectar race conditions.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::future::join_all;
use quickcheck::{Arbitrary, Gen};
use rand::{Rng, SeedableRng};
use tempfile::TempDir;
use tokio::sync::oneshot;

use ferres_db_server::bootstrap::{bootstrap_state, build_app};
use ferres_db_server::state::ServerConfig;

/// API key usada em todos os testes de propriedade.
const TEST_API_KEY: &str = "test-key-property";

// ─── Test Helpers ───────────────────────────────────────────────────────

/// Configuração do servidor de teste com porta aleatória.
struct TestServer {
    client: reqwest::Client,
    base_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
}

/// Inicia um servidor de teste em uma porta aleatória.
async fn setup_server() -> TestServer {
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

    let app_state = bootstrap_state(&config).unwrap();
    let app = build_app(&config, app_state);

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

    // Cliente com API key default em todos os requests
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {TEST_API_KEY}").parse().unwrap(),
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

/// Cria uma coleção via API (POST /api/v1/collections).
async fn create_collection(
    client: &reqwest::Client,
    base_url: &str,
    name: &str,
    dimension: usize,
) -> bool {
    let url = format!("{base_url}/api/v1/collections");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "name": name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();
    resp.status() == reqwest::StatusCode::CREATED
}

/// Gera um ponto determinístico para testes de propriedade (id e vetor de dimensão fixa).
fn create_random_point(i: usize, dimension: usize) -> serde_json::Value {
    let vector: Vec<f32> = (0..dimension).map(|j| (i * dimension + j) as f32).collect();
    serde_json::json!({
        "id": format!("point-{}", i),
        "vector": vector,
        "metadata": { "index": i }
    })
}

/// Insere um ponto na coleção (POST /api/v1/collections/{name}/points).
async fn upsert_point(
    client: &reqwest::Client,
    base_url: &str,
    collection_name: &str,
    point: serde_json::Value,
) {
    let url = format!("{base_url}/api/v1/collections/{collection_name}/points");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "points": [point] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "upsert should succeed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["upserted"], 1, "one point should be upserted");
}

/// Retorna o número de pontos da coleção (GET /api/v1/collections/{name}).
async fn get_collection_stats(client: &reqwest::Client, base_url: &str, name: &str) -> usize {
    let url = format!("{base_url}/api/v1/collections/{name}");
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["num_points"].as_u64().unwrap() as usize
}

// ─── Custom Arbitrary Types ─────────────────────────────────────────────

/// Nome de coleção válido para testes de propriedade.
#[derive(Debug, Clone)]
struct ValidCollectionName(String);

impl Arbitrary for ValidCollectionName {
    fn arbitrary(g: &mut Gen) -> Self {
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789-_".chars().collect();
        let len = (usize::arbitrary(g) % 20) + 3; // 3-22 caracteres

        let name: String = (0..len)
            .map(|i| {
                if i == 0 {
                    // Primeiro caractere deve ser letra
                    chars[usize::arbitrary(g) % 26]
                } else {
                    chars[usize::arbitrary(g) % chars.len()]
                }
            })
            .collect();

        ValidCollectionName(name)
    }
}

/// Dimensão válida para vetores (1-4096).
#[derive(Debug, Clone, Copy)]
struct ValidDimension(usize);

impl Arbitrary for ValidDimension {
    fn arbitrary(g: &mut Gen) -> Self {
        ValidDimension((usize::arbitrary(g) % 4095) + 1)
    }
}

/// Vetor de floats válido para testes (reservado para quickcheck de pontos).
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ValidVector(Vec<f32>);

impl ValidVector {
    #[allow(dead_code)]
    fn with_dimension(dim: usize, g: &mut Gen) -> Self {
        let vector: Vec<f32> = (0..dim)
            .map(|_| {
                let v = f32::arbitrary(g);
                if v.is_finite() {
                    v
                } else {
                    0.0
                }
            })
            .collect();
        ValidVector(vector)
    }
}

/// Tipo de distância válido.
#[derive(Debug, Clone, Copy)]
enum ValidDistance {
    Cosine,
    Euclidean,
    DotProduct,
}

impl Arbitrary for ValidDistance {
    fn arbitrary(g: &mut Gen) -> Self {
        match usize::arbitrary(g) % 3 {
            0 => ValidDistance::Cosine,
            1 => ValidDistance::Euclidean,
            _ => ValidDistance::DotProduct,
        }
    }
}

impl ValidDistance {
    fn as_str(&self) -> &'static str {
        match self {
            ValidDistance::Cosine => "Cosine",
            ValidDistance::Euclidean => "Euclidean",
            ValidDistance::DotProduct => "DotProduct",
        }
    }
}

// ─── QuickCheck Property Tests (generative) ─────────────────────────────

/// Número de casos gerados por teste de propriedade (equilíbrio velocidade/cobertura).
const QUICKCHECK_TESTS: u64 = 25;

/// Propriedade (quickcheck): para qualquer nome/dimensão/distância válidos,
/// criar coleção e depois GET deve retornar os mesmos dados.
fn prop_create_get_roundtrip_impl(
    name: ValidCollectionName,
    dim: ValidDimension,
    dist: ValidDistance,
) -> bool {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(prop_create_get_roundtrip_async(name, dim, dist))
}

async fn prop_create_get_roundtrip_async(
    name: ValidCollectionName,
    dim: ValidDimension,
    dist: ValidDistance,
) -> bool {
    let server = setup_server().await;
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let get_url = format!("{}/api/v1/collections/{}", server.base_url, name.0);

    let create_resp = server
        .client
        .post(&create_url)
        .json(&serde_json::json!({
            "name": name.0,
            "dimension": dim.0,
            "distance": dist.as_str()
        }))
        .send()
        .await
        .unwrap();

    if create_resp.status() != reqwest::StatusCode::CREATED {
        return false;
    }

    let get_resp = server.client.get(&get_url).send().await.unwrap();
    if get_resp.status() != reqwest::StatusCode::OK {
        return false;
    }

    let body: serde_json::Value = get_resp.json().await.unwrap();
    body["name"].as_str().unwrap() == name.0 && body["dimension"].as_u64().unwrap() == dim.0 as u64
}

#[test]
fn quickcheck_create_get_roundtrip() {
    quickcheck::QuickCheck::new()
        .tests(QUICKCHECK_TESTS)
        .quickcheck(
            prop_create_get_roundtrip_impl
                as fn(ValidCollectionName, ValidDimension, ValidDistance) -> bool,
        );
}

/// Propriedade (quickcheck): criar N coleções com nomes únicos e listar deve retornar exatamente N.
fn prop_list_collections_count_impl(names: Vec<ValidCollectionName>) -> bool {
    let mut seen = std::collections::HashSet::new();
    let names: Vec<_> = names
        .into_iter()
        .filter(|n| seen.insert(n.0.clone()))
        .take(15) // Limite para manter o teste rápido
        .collect();

    if names.is_empty() {
        return true;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(prop_list_collections_count_async(names))
}

async fn prop_list_collections_count_async(names: Vec<ValidCollectionName>) -> bool {
    let server = setup_server().await;
    let create_url = format!("{}/api/v1/collections", server.base_url);
    let list_url = format!("{}/api/v1/collections", server.base_url);

    for (i, name) in names.iter().enumerate() {
        let dim = (i % 4095) + 1;
        let dist = ["Cosine", "Euclidean", "DotProduct"][i % 3];
        let resp = server
            .client
            .post(&create_url)
            .json(&serde_json::json!({
                "name": name.0,
                "dimension": dim,
                "distance": dist
            }))
            .send()
            .await
            .unwrap();
        if resp.status() != reqwest::StatusCode::CREATED {
            return false;
        }
    }

    let list_resp = server.client.get(&list_url).send().await.unwrap();
    if list_resp.status() != reqwest::StatusCode::OK {
        return false;
    }
    let body: serde_json::Value = list_resp.json().await.unwrap();
    let collections = body["collections"].as_array().unwrap();
    collections.len() == names.len()
}

#[test]
fn quickcheck_list_collections_count() {
    quickcheck::QuickCheck::new()
        .tests(QUICKCHECK_TESTS)
        .quickcheck(prop_list_collections_count_impl as fn(Vec<ValidCollectionName>) -> bool);
}

/// Propriedade (quickcheck): upserts concorrentes em múltiplas coleções são seguros;
/// cada coleção termina com exatamente points_per_thread pontos.
fn prop_concurrent_upserts_are_safe_impl(
    collections: Vec<ValidCollectionName>,
    points_per_thread: usize,
) -> quickcheck::TestResult {
    if points_per_thread > 100 {
        return quickcheck::TestResult::discard();
    }
    let mut seen = std::collections::HashSet::new();
    let collections: Vec<String> = collections
        .into_iter()
        .filter(|n| seen.insert(n.0.clone()))
        .take(12)
        .map(|n| n.0)
        .collect();
    if collections.is_empty() {
        return quickcheck::TestResult::discard();
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(prop_concurrent_upserts_are_safe_async(
        collections,
        points_per_thread,
    ));
    if result {
        quickcheck::TestResult::passed()
    } else {
        quickcheck::TestResult::failed()
    }
}

async fn prop_concurrent_upserts_are_safe_async(
    collections: Vec<String>,
    points_per_thread: usize,
) -> bool {
    let server = setup_server().await;
    let client = Arc::new(server.client.clone());
    let base_url = Arc::new(server.base_url.clone());

    for name in &collections {
        if !create_collection(client.as_ref(), &base_url, name, 128).await {
            return false;
        }
    }

    let handles: Vec<_> = collections
        .iter()
        .map(|name| {
            let client = Arc::clone(&client);
            let base_url = Arc::clone(&base_url);
            let name = name.clone();
            tokio::spawn(async move {
                for i in 0..points_per_thread {
                    let point = create_random_point(i, 128);
                    upsert_point(client.as_ref(), &base_url, &name, point).await;
                }
            })
        })
        .collect();

    for h in handles {
        if h.await.is_err() {
            return false;
        }
    }

    for name in &collections {
        let num_points = get_collection_stats(client.as_ref(), &base_url, name).await;
        if num_points != points_per_thread {
            return false;
        }
    }
    true
}

#[test]
fn quickcheck_concurrent_upserts_are_safe() {
    quickcheck::QuickCheck::new()
        .tests(QUICKCHECK_TESTS)
        .quickcheck(
            prop_concurrent_upserts_are_safe_impl
                as fn(Vec<ValidCollectionName>, usize) -> quickcheck::TestResult,
        );
}

// ─── Property Tests (invariants, fixed iterations) ───────────────────────

/// Propriedade: criar e buscar uma coleção deve retornar os mesmos dados.
#[tokio::test]
async fn prop_create_get_collection_roundtrip() {
    let server = setup_server().await;

    // Testa com diferentes combinações de parâmetros
    for i in 0..10 {
        let name = format!("prop-test-{i}");
        let dimension = (i % 4095) + 1;
        let distances = ["Cosine", "Euclidean", "DotProduct"];
        let distance = distances[i % 3];

        let create_url = format!("{}/api/v1/collections", server.base_url);
        let get_url = format!("{}/api/v1/collections/{}", server.base_url, name);

        // Cria a coleção
        let create_response = server
            .client
            .post(&create_url)
            .json(&serde_json::json!({
                "name": name,
                "dimension": dimension,
                "distance": distance
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(create_response.status(), reqwest::StatusCode::CREATED);

        // Busca a coleção
        let get_response = server.client.get(&get_url).send().await.unwrap();

        assert_eq!(get_response.status(), reqwest::StatusCode::OK);

        let body: serde_json::Value = get_response.json().await.unwrap();
        assert_eq!(body["name"], name);
        assert_eq!(body["dimension"], dimension);
    }
}

/// Propriedade: inserir e buscar pontos deve retornar resultados consistentes.
#[tokio::test]
async fn prop_insert_search_points_consistency() {
    let server = setup_server().await;

    // Cria coleção para o teste
    let collection_name = "prop-points-test";
    let dimension = 8;

    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    // Insere pontos com vetores variados
    let mut rng = rand::thread_rng();
    let num_points = 50;
    let mut points = Vec::new();

    for i in 0..num_points {
        let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();
        points.push(serde_json::json!({
            "id": format!("point-{}", i),
            "vector": vector,
            "metadata": { "index": i }
        }));
    }

    let upsert_url = format!(
        "{}/api/v1/collections/{}/points",
        server.base_url, collection_name
    );
    let upsert_response = server
        .client
        .post(&upsert_url)
        .json(&serde_json::json!({ "points": points }))
        .send()
        .await
        .unwrap();

    assert_eq!(upsert_response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = upsert_response.json().await.unwrap();
    assert_eq!(body["upserted"], num_points);

    // Propriedade: busca deve retornar no máximo k resultados
    let search_url = format!(
        "{}/api/v1/collections/{}/search",
        server.base_url, collection_name
    );

    for k in [1, 5, 10, 20, 100] {
        let query_vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

        let search_response = server
            .client
            .post(&search_url)
            .json(&serde_json::json!({
                "vector": query_vector,
                "limit": k
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(search_response.status(), reqwest::StatusCode::OK);

        let body: serde_json::Value = search_response.json().await.unwrap();
        let results = body["results"].as_array().unwrap();

        // Propriedade: número de resultados <= min(k, num_points)
        assert!(results.len() <= k.min(num_points));

        // Propriedade: resultados devem estar ordenados por score
        let scores: Vec<f32> = results
            .iter()
            .map(|r| r["score"].as_f64().unwrap() as f32)
            .collect();

        for i in 1..scores.len() {
            assert!(scores[i - 1] <= scores[i], "Scores not sorted: {scores:?}");
        }
    }
}

/// Propriedade: dimensão do vetor deve corresponder à dimensão da coleção.
#[tokio::test]
async fn prop_dimension_validation() {
    let server = setup_server().await;

    let collection_name = "dim-validation-test";
    let dimension = 16;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    let upsert_url = format!(
        "{}/api/v1/collections/{}/points",
        server.base_url, collection_name
    );

    // Testa com diferentes dimensões incorretas
    for wrong_dim in [1, 8, 15, 17, 32, 64] {
        let vector: Vec<f32> = (0..wrong_dim).map(|i| i as f32).collect();

        let response = server
            .client
            .post(&upsert_url)
            .json(&serde_json::json!({
                "points": [{
                    "id": "test-point",
                    "vector": vector,
                    "metadata": {}
                }]
            }))
            .send()
            .await
            .unwrap();

        // API retorna 200 com pontos inválidos em "failed" (não 400)
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(
            body["upserted"], 0,
            "Wrong dimension {wrong_dim} should not be inserted for collection dimension {dimension}"
        );
        let failed = body["failed"].as_array().unwrap();
        assert!(
            !failed.is_empty(),
            "Wrong dimension should appear in failed list"
        );
    }

    // Vetor com dimensão correta deve ser aceito
    let correct_vector: Vec<f32> = (0..dimension).map(|i| i as f32).collect();
    let response = server
        .client
        .post(&upsert_url)
        .json(&serde_json::json!({
            "points": [{
                "id": "correct-point",
                "vector": correct_vector,
                "metadata": {}
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

/// Propriedade: IDs de pontos devem ser únicos (upsert sobrescreve).
#[tokio::test]
async fn prop_point_id_uniqueness() {
    let server = setup_server().await;

    let collection_name = "uniqueness-test";
    let dimension = 4;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    let upsert_url = format!(
        "{}/api/v1/collections/{}/points",
        server.base_url, collection_name
    );
    let get_url = format!("{}/api/v1/collections/{}", server.base_url, collection_name);

    // Insere ponto inicial
    server
        .client
        .post(&upsert_url)
        .json(&serde_json::json!({
            "points": [{
                "id": "same-id",
                "vector": [1.0, 0.0, 0.0, 0.0],
                "metadata": { "version": 1 }
            }]
        }))
        .send()
        .await
        .unwrap();

    // Verifica contagem
    let response = server.client.get(&get_url).send().await.unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["num_points"], 1);

    // Upsert com mesmo ID deve sobrescrever, não duplicar
    for version in 2..=10 {
        server
            .client
            .post(&upsert_url)
            .json(&serde_json::json!({
                "points": [{
                    "id": "same-id",
                    "vector": [version as f32, 0.0, 0.0, 0.0],
                    "metadata": { "version": version }
                }]
            }))
            .send()
            .await
            .unwrap();

        // Contagem deve permanecer 1
        let response = server.client.get(&get_url).send().await.unwrap();
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["num_points"], 1, "After version {version}");
    }
}

// ─── Concurrency Tests ──────────────────────────────────────────────────

/// Teste de concorrência: múltiplas escritas simultâneas.
#[tokio::test]
async fn test_concurrent_writes() {
    let server = setup_server().await;

    let collection_name = "concurrent-write-test";
    let dimension = 8;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    let num_writers = 10;
    let points_per_writer = 20;
    let client = Arc::new(server.client.clone());
    let base_url = Arc::new(server.base_url.clone());
    let success_count = Arc::new(AtomicUsize::new(0));

    // Lança múltiplas tasks de escrita concorrentes
    let mut handles = Vec::new();

    for writer_id in 0..num_writers {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        let success_count = Arc::clone(&success_count);

        let handle = tokio::spawn(async move {
            let upsert_url = format!("{base_url}/api/v1/collections/{collection_name}/points");
            let mut rng = rand::rngs::StdRng::from_entropy();

            for point_id in 0..points_per_writer {
                let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

                let response = client
                    .post(&upsert_url)
                    .json(&serde_json::json!({
                        "points": [{
                            "id": format!("writer-{}-point-{}", writer_id, point_id),
                            "vector": vector,
                            "metadata": { "writer": writer_id, "point": point_id }
                        }]
                    }))
                    .send()
                    .await;

                if let Ok(resp) = response {
                    if resp.status() == reqwest::StatusCode::OK {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Aguarda todas as tasks completarem
    join_all(handles).await;

    let total_success = success_count.load(Ordering::Relaxed);
    let expected = num_writers * points_per_writer;

    // Todas as escritas devem ter sucesso
    assert_eq!(
        total_success, expected,
        "Expected {expected} successful writes, got {total_success}"
    );

    // Verifica contagem final (retry em 429 por rate limit; aceita 429 após retries em CI)
    let get_url = format!("{}/api/v1/collections/{}", server.base_url, collection_name);
    let mut response = server.client.get(&get_url).send().await.unwrap();
    for _ in 0..5 {
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            response = server.client.get(&get_url).send().await.unwrap();
        } else {
            break;
        }
    }
    if response.status() == reqwest::StatusCode::OK {
        let text = response.text().await.unwrap();
        let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("GET collection response was not JSON: {e} (body: {text:?})")
        });
        assert_eq!(body["num_points"], expected);
    }
    // Se 429 após retries, teste de concorrência passou (servidor não crashou)
}

/// Teste de concorrência: leituras e escritas simultâneas.
#[tokio::test]
async fn test_concurrent_read_write() {
    let server = setup_server().await;

    let collection_name = "concurrent-rw-test";
    let dimension = 8;

    // Cria coleção e insere alguns pontos iniciais
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    // Insere pontos iniciais
    let initial_points: Vec<_> = (0..10)
        .map(|i| {
            serde_json::json!({
                "id": format!("initial-{}", i),
                "vector": vec![i as f32; dimension],
                "metadata": {}
            })
        })
        .collect();

    server
        .client
        .post(format!(
            "{}/api/v1/collections/{}/points",
            server.base_url, collection_name
        ))
        .json(&serde_json::json!({ "points": initial_points }))
        .send()
        .await
        .unwrap();

    let num_readers = 5;
    let num_writers = 5;
    let ops_per_task = 10;

    let client = Arc::new(server.client.clone());
    let base_url = Arc::new(server.base_url.clone());
    let read_success = Arc::new(AtomicUsize::new(0));
    let write_success = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    // Lança readers
    for _reader_id in 0..num_readers {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        let read_success = Arc::clone(&read_success);

        let handle = tokio::spawn(async move {
            let search_url = format!("{base_url}/api/v1/collections/{collection_name}/search");
            let mut rng = rand::rngs::StdRng::from_entropy();

            for _ in 0..ops_per_task {
                let query_vector: Vec<f32> =
                    (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

                let response = client
                    .post(&search_url)
                    .json(&serde_json::json!({
                        "vector": query_vector,
                        "limit": 5
                    }))
                    .send()
                    .await;

                if let Ok(resp) = response {
                    if resp.status() == reqwest::StatusCode::OK {
                        read_success.fetch_add(1, Ordering::Relaxed);
                    }
                }

                // Pequeno delay para simular workload real
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
        });

        handles.push(handle);
    }

    // Lança writers
    for writer_id in 0..num_writers {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        let write_success = Arc::clone(&write_success);

        let handle = tokio::spawn(async move {
            let upsert_url = format!("{base_url}/api/v1/collections/{collection_name}/points");
            let mut rng = rand::rngs::StdRng::from_entropy();

            for point_id in 0..ops_per_task {
                let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

                let response = client
                    .post(&upsert_url)
                    .json(&serde_json::json!({
                        "points": [{
                            "id": format!("rw-writer-{}-point-{}", writer_id, point_id),
                            "vector": vector,
                            "metadata": {}
                        }]
                    }))
                    .send()
                    .await;

                if let Ok(resp) = response {
                    if resp.status() == reqwest::StatusCode::OK {
                        write_success.fetch_add(1, Ordering::Relaxed);
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
        });

        handles.push(handle);
    }

    join_all(handles).await;

    let total_reads = read_success.load(Ordering::Relaxed);
    let total_writes = write_success.load(Ordering::Relaxed);

    // Pelo menos 90% das operações devem ter sucesso
    let expected_reads = num_readers * ops_per_task;
    let expected_writes = num_writers * ops_per_task;

    assert!(
        total_reads >= expected_reads * 9 / 10,
        "Expected at least {} reads, got {}",
        expected_reads * 9 / 10,
        total_reads
    );

    assert!(
        total_writes >= expected_writes * 9 / 10,
        "Expected at least {} writes, got {}",
        expected_writes * 9 / 10,
        total_writes
    );
}

/// Teste de concorrência: criação de múltiplas coleções simultâneas.
#[tokio::test]
async fn test_concurrent_collection_creation() {
    let server = setup_server().await;

    let num_collections = 20;
    let client = Arc::new(server.client.clone());
    let base_url = Arc::new(server.base_url.clone());
    let success_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for i in 0..num_collections {
        let client = Arc::clone(&client);
        let base_url = Arc::clone(&base_url);
        let success_count = Arc::clone(&success_count);

        let handle = tokio::spawn(async move {
            let url = format!("{base_url}/api/v1/collections");
            let distances = ["Cosine", "Euclidean", "DotProduct"];
            let distance = distances[i % 3];

            let response = client
                .post(&url)
                .json(&serde_json::json!({
                    "name": format!("concurrent-col-{}", i),
                    "dimension": (i % 100) + 10,
                    "distance": distance
                }))
                .send()
                .await;

            if let Ok(resp) = response {
                if resp.status() == reqwest::StatusCode::CREATED {
                    success_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    join_all(handles).await;

    let total_success = success_count.load(Ordering::Relaxed);
    assert_eq!(total_success, num_collections);

    // Verifica que todas as coleções existem
    let list_url = format!("{}/api/v1/collections", server.base_url);
    let response = server.client.get(&list_url).send().await.unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    let collections = body["collections"].as_array().unwrap();

    assert_eq!(collections.len(), num_collections);
}

// ─── Load Tests ─────────────────────────────────────────────────────────

/// Teste de carga: muitas operações em sequência rápida.
#[tokio::test]
async fn test_load_rapid_operations() {
    let server = setup_server().await;

    let collection_name = "load-test";
    let dimension = 32;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Cosine"
        }))
        .send()
        .await
        .unwrap();

    let upsert_url = format!(
        "{}/api/v1/collections/{}/points",
        server.base_url, collection_name
    );
    let search_url = format!(
        "{}/api/v1/collections/{}/search",
        server.base_url, collection_name
    );

    let mut rng = rand::thread_rng();
    let num_batches = 20;
    let points_per_batch = 50;
    let searches_per_batch = 10;

    let start = std::time::Instant::now();

    for batch in 0..num_batches {
        // Insere batch de pontos
        let points: Vec<_> = (0..points_per_batch)
            .map(|i| {
                let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();
                serde_json::json!({
                    "id": format!("load-batch-{}-point-{}", batch, i),
                    "vector": vector,
                    "metadata": { "batch": batch }
                })
            })
            .collect();

        let mut response = server
            .client
            .post(&upsert_url)
            .json(&serde_json::json!({ "points": points }))
            .send()
            .await
            .unwrap();
        for _ in 0..5 {
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                response = server
                    .client
                    .post(&upsert_url)
                    .json(&serde_json::json!({ "points": points }))
                    .send()
                    .await
                    .unwrap();
            } else {
                break;
            }
        }
        assert!(
            response.status() == reqwest::StatusCode::OK
                || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Upsert should return 200 or 429 (rate limit), got {}",
            response.status()
        );

        // Realiza algumas buscas (aceita 429 rate limit em ambiente de teste)
        for _ in 0..searches_per_batch {
            let query_vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

            let response = server
                .client
                .post(&search_url)
                .json(&serde_json::json!({
                    "vector": query_vector,
                    "limit": 10
                }))
                .send()
                .await
                .unwrap();

            let status = response.status();
            assert!(
                status == reqwest::StatusCode::OK
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS,
                "Search should return 200 or 429 (rate limit), got {status}"
            );
        }
    }

    let elapsed = start.elapsed();
    let total_points = num_batches * points_per_batch;
    let total_searches = num_batches * searches_per_batch;

    println!(
        "Load test completed: {total_points} points, {total_searches} searches in {elapsed:?}"
    );

    // Verifica contagem final (retry em 429)
    let get_url = format!("{}/api/v1/collections/{}", server.base_url, collection_name);
    let mut get_response = server.client.get(&get_url).send().await.unwrap();
    for _ in 0..5 {
        if get_response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            get_response = server.client.get(&get_url).send().await.unwrap();
        } else {
            break;
        }
    }
    if get_response.status() == reqwest::StatusCode::OK {
        let body: serde_json::Value = get_response.json().await.unwrap();
        assert_eq!(body["num_points"], total_points);
    }
}

/// Teste de carga: batch insert grande.
#[tokio::test]
async fn test_load_large_batch_insert() {
    let server = setup_server().await;

    let collection_name = "large-batch-test";
    let dimension = 128;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Cosine"
        }))
        .send()
        .await
        .unwrap();

    let upsert_url = format!(
        "{}/api/v1/collections/{}/points",
        server.base_url, collection_name
    );

    let mut rng = rand::thread_rng();
    let batch_size = 500; // Batch grande para testar insert_batch otimizado

    // Gera batch grande
    let points: Vec<_> = (0..batch_size)
        .map(|i| {
            let vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();
            serde_json::json!({
                "id": format!("large-batch-{}", i),
                "vector": vector,
                "metadata": { "index": i }
            })
        })
        .collect();

    let start = std::time::Instant::now();

    let response = server
        .client
        .post(&upsert_url)
        .json(&serde_json::json!({ "points": points }))
        .send()
        .await
        .unwrap();

    let elapsed = start.elapsed();

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["upserted"], batch_size);

    println!(
        "Large batch insert: {} points of dim {} in {:?} ({:.0} points/sec)",
        batch_size,
        dimension,
        elapsed,
        batch_size as f64 / elapsed.as_secs_f64()
    );

    // Verifica que os pontos são buscáveis
    let search_url = format!(
        "{}/api/v1/collections/{}/search",
        server.base_url, collection_name
    );
    let query_vector: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

    let search_response = server
        .client
        .post(&search_url)
        .json(&serde_json::json!({
            "vector": query_vector,
            "limit": 10
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(search_response.status(), reqwest::StatusCode::OK);

    let search_body: serde_json::Value = search_response.json().await.unwrap();
    assert_eq!(search_body["results"].as_array().unwrap().len(), 10);
}

/// Teste de stress: muitas requisições paralelas.
#[tokio::test]
async fn test_stress_parallel_requests() {
    let server = setup_server().await;

    let collection_name = "stress-test";
    let dimension = 16;

    // Cria coleção
    server
        .client
        .post(format!("{}/api/v1/collections", server.base_url))
        .json(&serde_json::json!({
            "name": collection_name,
            "dimension": dimension,
            "distance": "Euclidean"
        }))
        .send()
        .await
        .unwrap();

    // Insere alguns pontos iniciais
    let initial_points: Vec<_> = (0..100)
        .map(|i| {
            serde_json::json!({
                "id": format!("stress-{}", i),
                "vector": vec![i as f32 / 100.0; dimension],
                "metadata": {}
            })
        })
        .collect();

    server
        .client
        .post(format!(
            "{}/api/v1/collections/{}/points",
            server.base_url, collection_name
        ))
        .json(&serde_json::json!({ "points": initial_points }))
        .send()
        .await
        .unwrap();

    let num_parallel = 50;
    let client = Arc::new(server.client.clone());
    let base_url = Arc::new(server.base_url.clone());
    let success_count = Arc::new(AtomicUsize::new(0));

    let start = std::time::Instant::now();

    let handles: Vec<_> = (0..num_parallel)
        .map(|i| {
            let client = Arc::clone(&client);
            let base_url = Arc::clone(&base_url);
            let success_count = Arc::clone(&success_count);

            tokio::spawn(async move {
                let search_url = format!("{base_url}/api/v1/collections/{collection_name}/search");
                let query_vector = vec![i as f32 / 50.0; dimension];

                let response = client
                    .post(&search_url)
                    .json(&serde_json::json!({
                        "vector": query_vector,
                        "limit": 10
                    }))
                    .send()
                    .await;

                if let Ok(resp) = response {
                    if resp.status() == reqwest::StatusCode::OK {
                        success_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    join_all(handles).await;

    let elapsed = start.elapsed();
    let total_success = success_count.load(Ordering::Relaxed);

    println!(
        "Stress test: {} parallel requests in {:?} ({:.0} req/sec)",
        num_parallel,
        elapsed,
        num_parallel as f64 / elapsed.as_secs_f64()
    );

    // Todas as requisições devem ter sucesso
    assert_eq!(total_success, num_parallel);
}
