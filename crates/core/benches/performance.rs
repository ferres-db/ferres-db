use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use ferres_db_core::{
    CollectionConfig, DistanceMetric, Point, VectorDB,
};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;

/// Carrega pontos de um arquivo JSONL.
fn load_points_from_jsonl(file_path: &PathBuf) -> Vec<Point> {
    let file = File::open(file_path).expect("failed to open corpus file");
    let reader = BufReader::new(file);
    let mut points = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line.expect("failed to read line");
        if line.trim().is_empty() {
            continue;
        }

        let json: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("invalid JSON at line {}: {}", line_num + 1, e));

        let id = json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("missing 'id' at line {}", line_num + 1))
            .unwrap()
            .to_string();

        let vector: Vec<f32> = json
            .get("vector")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("missing 'vector' at line {}", line_num + 1))
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();

        let metadata = json.get("metadata").cloned().unwrap_or(serde_json::Value::Null);

        let point = Point::new(id, vector, metadata)
            .unwrap_or_else(|e| panic!("failed to create point at line {}: {}", line_num + 1, e));
        points.push(point);
    }

    points
}

/// Retorna o caminho para um arquivo de corpus.
fn corpus_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../tests/fixtures");
    path.push(name);
    path
}

// ─── Benchmark de Indexação ─────────────────────────────────────────────

fn benchmark_indexing(c: &mut Criterion) {
    let mut group = c.benchmark_group("indexing");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    let corpus_sizes = vec![
        ("corpus_1k.jsonl", 1000),
        ("corpus_10k.jsonl", 10000),
        ("corpus_100k.jsonl", 100000),
    ];

    for (filename, _expected_size) in corpus_sizes {
        let corpus_file = corpus_path(filename);
        
        // Verifica se o arquivo existe, se não, pula o benchmark
        if !corpus_file.exists() {
            eprintln!("⚠️  Arquivo não encontrado: {:?}", corpus_file);
            eprintln!("   Execute: python tests/fixtures/generate_corpus.py");
            continue;
        }

        let points = load_points_from_jsonl(&corpus_file);
        let actual_size = points.len();
        
        if actual_size == 0 {
            eprintln!("⚠️  Arquivo vazio: {:?}", corpus_file);
            continue;
        }

        // Determina dimensão do primeiro ponto
        let dimension = points[0].dimension();

        group.throughput(Throughput::Elements(actual_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_points", actual_size)),
            &points,
            |b, points| {
                b.iter(|| {
                    // Cria um diretório temporário para cada iteração
                    let temp_dir = std::env::temp_dir().join(format!(
                        "ferres_bench_{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    ));
                    
                    let mut db = VectorDB::new(temp_dir.clone()).unwrap();
                    
                    let config = CollectionConfig {
                        name: "bench_collection".into(),
                        dimension,
                        distance: DistanceMetric::Cosine,
                        hnsw: Default::default(),
                        search_cache_size: 0,
                        enable_bm25: false,
                        bm25_text_field: "text".to_string(),
                        quantization: Default::default(),
                    };
                    
                    db.create_collection(config).unwrap();
                    db.upsert_points("bench_collection", black_box(points.clone()))
                        .unwrap();
                    
                    // Limpa o diretório temporário
                    let _ = std::fs::remove_dir_all(&temp_dir);
                });
            },
        );
    }

    group.finish();
}

// ─── Benchmark de Busca ──────────────────────────────────────────────────

fn benchmark_search(c: &mut Criterion) {
    let corpus_file = corpus_path("corpus_10k.jsonl");
    
    if !corpus_file.exists() {
        eprintln!("⚠️  Arquivo não encontrado: {:?}", corpus_file);
        eprintln!("   Execute: python tests/fixtures/generate_corpus.py");
        return;
    }

    let points = load_points_from_jsonl(&corpus_file);
    
    if points.is_empty() {
        eprintln!("⚠️  Arquivo vazio: {:?}", corpus_file);
        return;
    }

    let dimension = points[0].dimension();
    let num_queries = 100;

    // Prepara o banco de dados com os pontos
    let temp_dir = std::env::temp_dir().join(format!(
        "ferres_search_bench_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let mut db = VectorDB::new(temp_dir.clone()).unwrap();
    let config = CollectionConfig {
        name: "search_bench".into(),
        dimension,
        distance: DistanceMetric::Cosine,
        hnsw: Default::default(),
        search_cache_size: 0,
        enable_bm25: false,
        bm25_text_field: "text".to_string(),
        quantization: Default::default(),
    };
    db.create_collection(config).unwrap();
    db.upsert_points("search_bench", points.clone()).unwrap();

    // Gera queries aleatórias (usa vetores dos próprios pontos)
    let queries: Vec<Vec<f32>> = points
        .iter()
        .take(num_queries)
        .map(|p| p.vector.clone())
        .collect();

    let mut group = c.benchmark_group("search");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(10));

    // Benchmark de uma única query (Criterion mede automaticamente)
    let query = queries[0].clone();
    group.bench_function("search_10k_single_query", |b| {
        b.iter(|| {
            let _results = black_box(
                db.search("search_bench", black_box(query.clone()), 10)
                    .unwrap(),
            );
        });
    });

    // Benchmark de múltiplas queries (100 queries)
    group.bench_with_input(
        BenchmarkId::from_parameter("search_10k_100_queries"),
        &queries,
        |b, queries| {
            b.iter(|| {
                for query in queries {
                    let _results = black_box(
                        db.search("search_bench", black_box(query.clone()), 10)
                            .unwrap(),
                    );
                }
            });
        },
    );

    group.finish();

    // Calcula e reporta percentis manualmente (execução separada)
    println!("\n📊 Calculando percentis de latência (100 queries)...");
    let mut latencies = Vec::new();
    for query in &queries {
        let start = std::time::Instant::now();
        let _results = db.search("search_bench", query.clone(), 10).unwrap();
        latencies.push(start.elapsed().as_micros() as f64);
    }

    if !latencies.is_empty() {
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() * 95) / 100];
        let p99 = latencies[(latencies.len() * 99) / 100];
        
        println!("   P50: {:.2} μs", p50);
        println!("   P95: {:.2} μs", p95);
        println!("   P99: {:.2} μs", p99);
    }

    // Limpa o diretório temporário
    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ─── Benchmark de Upsert ─────────────────────────────────────────────────

fn benchmark_upsert(c: &mut Criterion) {
    let base_corpus = corpus_path("corpus_10k.jsonl");
    let new_points_file = corpus_path("corpus_1k.jsonl");
    
    if !base_corpus.exists() || !new_points_file.exists() {
        eprintln!("⚠️  Arquivos não encontrados:");
        eprintln!("   Base: {:?}", base_corpus);
        eprintln!("   Novos pontos: {:?}", new_points_file);
        eprintln!("   Execute: python tests/fixtures/generate_corpus.py");
        return;
    }

    let base_points = load_points_from_jsonl(&base_corpus);
    let new_points = load_points_from_jsonl(&new_points_file);
    
    if base_points.is_empty() || new_points.is_empty() {
        eprintln!("⚠️  Arquivos vazios");
        return;
    }

    let dimension = base_points[0].dimension();

    let mut group = c.benchmark_group("upsert");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));
    group.throughput(Throughput::Elements(new_points.len() as u64));

    group.bench_function("upsert_1k_into_10k", |b| {
        b.iter(|| {
            // Cria um novo banco para cada iteração
            let temp_dir = std::env::temp_dir().join(format!(
                "ferres_upsert_bench_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));

            let mut db = VectorDB::new(temp_dir.clone()).unwrap();
            let config = CollectionConfig {
                name: "upsert_bench".into(),
                dimension,
                distance: DistanceMetric::Cosine,
                hnsw: Default::default(),
                search_cache_size: 0,
                enable_bm25: false,
                bm25_text_field: "text".to_string(),
                quantization: Default::default(),
            };
            db.create_collection(config).unwrap();

            // Indexa os pontos base
            db.upsert_points("upsert_bench", black_box(base_points.clone()))
                .unwrap();

            // Mede o tempo de adicionar novos pontos
            db.upsert_points("upsert_bench", black_box(new_points.clone()))
                .unwrap();

            // Limpa o diretório temporário
            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    });

    group.finish();
}

// ─── Benchmark de SQ8 Quantization ──────────────────────────────────────

fn benchmark_sq8(c: &mut Criterion) {
    use rand::Rng;

    let mut rng = rand::thread_rng();
    let mut group = c.benchmark_group("sq8_quantization");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    for n in [10_000usize, 100_000] {
        let dim = 384;
        let k = 10;

        // Gera vetores aleatórios
        let points: Vec<ferres_db_core::Point> = (0..n)
            .map(|i| {
                let vector: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0_f32..1.0)).collect();
                ferres_db_core::Point {
                    id: format!("v{i}"),
                    vector,
                    metadata: serde_json::Value::Null,
                    created_at: 0,
                }
            })
            .collect();

        let queries: Vec<Vec<f32>> = (0..100)
            .map(|_| (0..dim).map(|_| rng.gen_range(-1.0_f32..1.0)).collect())
            .collect();

        // Benchmark SQ8 build + search
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("sq8_build_{n}")),
            &points,
            |b, points| {
                b.iter(|| {
                    let sq_config = ferres_db_core::ScalarQuantizationConfig {
                        dtype: ferres_db_core::ScalarType::Int8,
                        always_ram: false,
                        quantile: 99.5,
                    };
                    let mut index = ferres_db_core::QuantizedHnswIndex::new(
                        ferres_db_core::DistanceMetric::Euclidean,
                        ferres_db_core::HnswConfig {
                            max_nb_connection: 16,
                            max_elements: n + 100,
                            max_layer: 16,
                            ef_construction: 200,
                            ef_search: 50,
                        },
                        sq_config,
                    );
                    index.build(black_box(points)).unwrap();
                });
            },
        );

        // Compare recall: f32 vs SQ8
        // Build indices once for search benchmarks
        let hnsw_config = ferres_db_core::HnswConfig {
            max_nb_connection: 16,
            max_elements: n + 100,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 50,
        };

        let mut normal_index = ferres_db_core::HnswIndex::new(
            ferres_db_core::DistanceMetric::Euclidean,
            hnsw_config.clone(),
        );
        normal_index.build(&points).unwrap();

        let sq_config = ferres_db_core::ScalarQuantizationConfig {
            dtype: ferres_db_core::ScalarType::Int8,
            always_ram: false,
            quantile: 99.5,
        };
        let mut sq_index = ferres_db_core::QuantizedHnswIndex::new(
            ferres_db_core::DistanceMetric::Euclidean,
            hnsw_config,
            sq_config,
        );
        sq_index.build(&points).unwrap();

        // Benchmark f32 search
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("f32_search_{n}")),
            &queries,
            |b, queries| {
                b.iter(|| {
                    for q in queries {
                        let _ = black_box(normal_index.search(q, k).unwrap());
                    }
                });
            },
        );

        // Benchmark SQ8 search
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("sq8_search_{n}")),
            &queries,
            |b, queries| {
                b.iter(|| {
                    for q in queries {
                        let _ = black_box(sq_index.search(q, k).unwrap());
                    }
                });
            },
        );

        // Print recall comparison
        println!("\n📊 SQ8 Recall@{k} comparison ({n} vectors, dim={dim}):");
        let mut total_overlap = 0usize;
        let mut total_possible = 0usize;
        for q in &queries {
            let normal_results = normal_index.search(q, k).unwrap();
            let sq_results = sq_index.search(q, k).unwrap();

            let normal_ids: std::collections::HashSet<&str> =
                normal_results.iter().map(|r| r.0.as_str()).collect();
            let sq_ids: std::collections::HashSet<&str> =
                sq_results.iter().map(|r| r.0.as_str()).collect();

            total_overlap += normal_ids.intersection(&sq_ids).count();
            total_possible += k.min(normal_results.len());
        }
        let recall = if total_possible > 0 {
            total_overlap as f64 / total_possible as f64
        } else {
            0.0
        };
        println!("   Recall@{k}: {recall:.3} ({total_overlap}/{total_possible})");

        // Print memory comparison
        let f32_mem = n * dim * 4;
        let u8_mem = n * dim * 1;
        println!(
            "   Memory: f32={:.1}MB, SQ8={:.1}MB ({:.1}x compression)",
            f32_mem as f64 / 1024.0 / 1024.0,
            u8_mem as f64 / 1024.0 / 1024.0,
            f32_mem as f64 / u8_mem as f64
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_indexing, benchmark_search, benchmark_upsert, benchmark_sq8);
criterion_main!(benches);

