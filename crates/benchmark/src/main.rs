//! FerresDB — Ferramenta oficial de Benchmark e Stress Test
//!
//! Modos: ingest (write stress), search (read stress), chaos (mixed).
//! Suporta API key do FerresDB (docs/api.md) e embeddings OpenAI para ingest/busca realistas.

use clap::{Parser, Subcommand};
use ferres_db_sdk::{FerresDbClient, PointInput};
use hdrhistogram::Histogram;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rand::{Rng, SeedableRng};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;

const DEFAULT_URL: &str = "http://localhost:3000";
const DEFAULT_COLLECTION: &str = "bench";
const BATCH_SIZE: usize = 500;
const OPENAI_EMBED_API: &str = "https://api.openai.com/v1/embeddings";
const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";

#[derive(Parser)]
#[command(name = "ferres-bench")]
#[command(about = "Benchmark e stress test oficial para o FerresDB", long_about = None)]
struct Cli {
    #[arg(long, default_value = DEFAULT_URL)]
    url: String,

    /// API key do FerresDB (Authorization: Bearer). Ver docs/api.md. Opcional se o servidor não exigir. Env: FERRESDB_API_KEY.
    #[arg(long)]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Write stress: cria coleção, gera textos, embeddings OpenAI (ou vetores aleatórios) e insere
    Ingest {
        #[arg(long, default_value = "10000")]
        vectors: u32,

        #[arg(long, default_value = "50")]
        concurrency: usize,

        #[arg(long, default_value = DEFAULT_COLLECTION)]
        collection: String,

        /// Dimensão (usado apenas sem --openai-api-key). Com OpenAI, a dimensão vem do model.
        #[arg(long)]
        dim: Option<u32>,

        /// OpenAI API key para embeddings. Se não informado, usa vetores aleatórios. Env: OPENAI_API_KEY.
        #[arg(long)]
        openai_api_key: Option<String>,

        #[arg(long, default_value = DEFAULT_EMBEDDING_MODEL)]
        embedding_model: String,
    },

    /// Read stress: buscas vetoriais (query → embedding OpenAI ou vetor aleatório) por duração
    Search {
        #[arg(long, default_value = "60s", value_parser = parse_duration)]
        duration: Duration,

        #[arg(long, default_value = "100")]
        concurrency: usize,

        #[arg(long, default_value = DEFAULT_COLLECTION)]
        collection: String,

        #[arg(long)]
        dim: Option<u32>,

        /// Env: OPENAI_API_KEY.
        #[arg(long)]
        openai_api_key: Option<String>,

        #[arg(long, default_value = DEFAULT_EMBEDDING_MODEL)]
        embedding_model: String,

        /// Número de query vectors a pré-computar (pool size).
        #[arg(long, default_value = "200")]
        num_queries: usize,

        /// Número de resultados por busca.
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Warmup requests (não contabilizadas).
        #[arg(long, default_value = "10")]
        warmup: usize,
    },

    /// Chaos: escritas, leituras e criações de coleção simultâneas
    Chaos {
        #[arg(long, default_value = "30s", value_parser = parse_duration)]
        duration: Duration,

        #[arg(long, default_value = "20")]
        writers: usize,

        #[arg(long, default_value = "50")]
        readers: usize,

        #[arg(long, default_value = "768")]
        dim: u32,
    },
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| e.to_string())
}

fn format_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + (s.len() - 1) / 3);
    let chars: Vec<char> = s.chars().rev().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out.chars().rev().collect()
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.2}s", secs)
    } else {
        format!("{:.0}ms", secs * 1000.0)
    }
}

/// Relatório final em Markdown (copiável para GitHub/LinkedIn)
fn print_markdown_report(mode: &str, metrics: &[(String, String)]) {
    println!("\n---\n## FerresDB Benchmark Report — {mode}\n");
    println!("| Metric | Value |");
    println!("|--------|-------|");
    for (k, v) in metrics {
        println!("| {k} | {v} |");
    }
    println!("\n---\n");
}

fn random_vector(dim: u32, rng: &mut impl Rng) -> Vec<f32> {
    (0..dim).map(|_| rng.gen_range(-1.0..=1.0)).collect()
}

/// Dimensão do vetor para modelos OpenAI conhecidos (docs OpenAI).
fn embedding_dimension_for_model(model: &str) -> u32 {
    match model {
        "text-embedding-3-small" | "text-embedding-ada-002" => 1536,
        "text-embedding-3-large" => 3072,
        _ => 1536,
    }
}

/// Chama a API de embeddings da OpenAI (batch). Input até 2048 textos por request.
async fn openai_embed_batch(
    texts: &[String],
    api_key: &str,
    model: &str,
    client: &reqwest::Client,
) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::json!({ "model": model, "input": texts });
    let resp = client
        .post(OPENAI_EMBED_API)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI API error: {}", msg).into());
    }
    let data: serde_json::Value = resp.json().await?;
    let data_arr = data
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or("OpenAI response missing 'data'")?;
    let mut by_index: Vec<(u64, Vec<f32>)> = data_arr
        .iter()
        .filter_map(|o| {
            let idx = o.get("index")?.as_u64()?;
            let emb = o.get("embedding")?.as_array()?;
            let v: Vec<f32> = emb
                .iter()
                .filter_map(|e| e.as_f64().map(|f| f as f32))
                .collect();
            Some((idx, v))
        })
        .collect();
    by_index.sort_by_key(|(i, _)| *i);
    Ok(by_index.into_iter().map(|(_, v)| v).collect())
}

/// Um único texto → vetor (chamada à API OpenAI). Usado apenas fora do benchmark search (ex.: ingest).
#[allow(dead_code)]
async fn openai_embed_one(
    text: &str,
    api_key: &str,
    model: &str,
    client: &reqwest::Client,
) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let body = serde_json::json!({ "model": model, "input": text });
    let resp = client
        .post(OPENAI_EMBED_API)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI API error: {}", msg).into());
    }
    let data: serde_json::Value = resp.json().await?;
    let emb = data
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|o| o.get("embedding").and_then(|e| e.as_array()))
        .ok_or("OpenAI response missing embedding")?;
    Ok(emb
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect())
}

/// Cliente HTTP único com connection pooling (keep-alive) para todo o benchmark.
fn make_http_client(
    api_key: Option<&str>,
) -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = reqwest::Client::builder()
        .pool_max_idle_per_host(500)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(30));
    if let Some(key) = api_key {
        let mut headers = reqwest::header::HeaderMap::new();
        let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key))
            .map_err(|_| "invalid API key for header")?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
        builder = builder.default_headers(headers);
    }
    Ok(builder.build()?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    let url = std::env::var("FERRESDB_URL").unwrap_or(cli.url.clone());
    let api_key = cli
        .api_key
        .or_else(|| std::env::var("FERRESDB_API_KEY").ok());
    let http_client = make_http_client(api_key.as_deref())?;
    let client = FerresDbClient::new_with_client(&url, http_client.clone());

    match &cli.command {
        Commands::Ingest {
            vectors,
            concurrency,
            collection,
            dim,
            openai_api_key,
            embedding_model,
        } => {
            let openai_key = openai_api_key
                .clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok());
            let dim = dim.or(openai_key
                .as_ref()
                .map(|_| embedding_dimension_for_model(embedding_model)));
            let dim = dim.unwrap_or(1536);
            run_ingest(
                client,
                &http_client,
                *vectors,
                dim,
                *concurrency,
                collection,
                openai_key.as_deref(),
                embedding_model,
            )
            .await?;
        }
        Commands::Search {
            duration,
            concurrency,
            collection,
            dim,
            openai_api_key,
            embedding_model,
            num_queries,
            limit,
            warmup,
        } => {
            let openai_key = openai_api_key
                .clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok());
            let dim = dim.or(openai_key
                .as_ref()
                .map(|_| embedding_dimension_for_model(embedding_model)));
            let dim = dim.unwrap_or(1536);
            run_search(
                client,
                &url,
                &http_client,
                *duration,
                *concurrency,
                collection,
                dim,
                openai_key.as_deref(),
                embedding_model,
                *num_queries,
                *limit,
                *warmup,
            )
            .await?;
        }
        Commands::Chaos {
            duration,
            writers,
            readers,
            dim,
        } => {
            run_chaos(client, *duration, *writers, *readers, *dim).await?;
        }
    }

    Ok(())
}

async fn run_ingest(
    client: FerresDbClient,
    http_client: &reqwest::Client,
    total_vectors: u32,
    dim: u32,
    concurrency: usize,
    collection: &str,
    openai_api_key: Option<&str>,
    embedding_model: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("FerresDB Ingest Benchmark");
    println!(
        "  vectors: {}, dim: {}, concurrency: {}",
        total_vectors, dim, concurrency
    );
    println!("  collection: {}", collection);
    if openai_api_key.is_some() {
        println!("  embeddings: OpenAI ({})", embedding_model);
    } else {
        println!("  embeddings: random vectors");
    }

    match client
        .create_collection(collection, dim, "Cosine", false)
        .await
    {
        Ok(_) => println!(
            "  Collection '{}' created (dim={}, distance=Cosine).",
            collection, dim
        ),
        Err(e) => {
            let msg = e.to_string();
            let already_exists = msg.to_lowercase().contains("already exists")
                || msg.contains("409")
                || msg.to_lowercase().contains("collection_already_exists");
            if already_exists {
                println!("  Collection '{}' already exists, using it.", collection);
            } else {
                return Err(format!("Failed to create collection '{}': {}. Ensure the server is up and --api-key is set if required.", collection, e).into());
            }
        }
    }

    let total_batches = (total_vectors as usize + BATCH_SIZE - 1) / BATCH_SIZE;
    let batches_per_worker = (total_batches + concurrency - 1) / concurrency;

    let mp = MultiProgress::new();
    let pb = mp.add(ProgressBar::new(total_vectors as u64));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} vectors ({msg})")
            .unwrap()
            .progress_chars("=>-"),
    );

    let inserted = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    if let Some(api_key) = openai_api_key {
        // OpenAI flow: generate texts, embed in batches (OpenAI allows batch input), then upsert
        const EMBED_BATCH: usize = 100;
        let mut handles = Vec::with_capacity(concurrency);
        for w in 0..concurrency {
            let client = client.clone();
            let http_client = http_client.clone();
            let coll = collection.to_string();
            let key = api_key.to_string();
            let model = embedding_model.to_string();
            let inserted = inserted.clone();
            let pb = pb.clone();
            let batch_start = w * batches_per_worker;
            let batch_end = ((w + 1) * batches_per_worker).min(total_batches);

            let h = tokio::spawn(async move {
                for b in batch_start..batch_end {
                    let base = b * BATCH_SIZE;
                    let count = (BATCH_SIZE).min(total_vectors as usize - base);
                    if count == 0 {
                        break;
                    }
                    let texts: Vec<String> = (0..count)
                        .map(|i| format!("Document {} content for benchmark.", base + i))
                        .collect();
                    let mut points_vec = Vec::with_capacity(count);
                    for (chunk_start, chunk) in texts.chunks(EMBED_BATCH).enumerate() {
                        let chunk_texts: Vec<String> = chunk.to_vec();
                        match openai_embed_batch(&chunk_texts, &key, &model, &http_client).await {
                            Ok(vectors) => {
                                for (i, vector) in vectors.into_iter().enumerate() {
                                    let idx = chunk_start * EMBED_BATCH + i;
                                    if idx < count {
                                        points_vec.push(PointInput {
                                            id: format!("doc-{}", base + idx),
                                            vector,
                                            metadata: Some(serde_json::json!({ "text": chunk_texts.get(i).cloned().unwrap_or_default() })),
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                pb.println(format!("OpenAI embed error: {}", e));
                            }
                        }
                    }
                    if points_vec.is_empty() {
                        continue;
                    }
                    match client.upsert_points(&coll, &points_vec).await {
                        Ok(res) => {
                            let n = res.upserted as u64;
                            inserted.fetch_add(n, Ordering::Relaxed);
                            pb.inc(n);
                        }
                        Err(e) => {
                            pb.println(format!("Upsert error: {}", e));
                        }
                    }
                }
            });
            handles.push(h);
        }
        for h in handles {
            let _ = h.await;
        }
    } else {
        let total_batches = (total_vectors as usize + BATCH_SIZE - 1) / BATCH_SIZE;
        let batches_per_worker = (total_batches + concurrency - 1) / concurrency;
        let mut handles = Vec::with_capacity(concurrency);
        for w in 0..concurrency {
            let client = client.clone();
            let coll = collection.to_string();
            let inserted = inserted.clone();
            let pb = pb.clone();
            let batch_start = w * batches_per_worker;
            let batch_end = ((w + 1) * batches_per_worker).min(total_batches);

            let h = tokio::spawn(async move {
                let mut rng = rand::rngs::StdRng::from_entropy();
                for b in batch_start..batch_end {
                    let base = b * BATCH_SIZE;
                    let count = (BATCH_SIZE).min(total_vectors as usize - base);
                    if count == 0 {
                        break;
                    }
                    let points: Vec<PointInput> = (0..count)
                        .map(|i| {
                            let id = format!("bench-{}", base + i);
                            let vector = random_vector(dim, &mut rng);
                            PointInput {
                                id,
                                vector,
                                metadata: None,
                            }
                        })
                        .collect();
                    match client.upsert_points(&coll, &points).await {
                        Ok(res) => {
                            inserted.fetch_add(res.upserted as u64, Ordering::Relaxed);
                            pb.inc(res.upserted as u64);
                        }
                        Err(e) => pb.println(format!("Upsert error: {}", e)),
                    }
                }
            });
            handles.push(h);
        }
        for h in handles {
            let _ = h.await;
        }
    }

    pb.finish_with_message("done");

    let elapsed = start.elapsed();
    let total = inserted.load(Ordering::Relaxed);
    let throughput = total as f64 / elapsed.as_secs_f64();

    println!("\nIngest complete.");
    println!("  Total vectors: {}", format_num(total));
    println!("  Duration: {}", format_duration(elapsed));
    println!("  Throughput: {} vectors/s", format_num(throughput as u64));

    let metrics = vec![
        ("Mode".to_string(), "Ingest (Write Stress)".to_string()),
        ("Total Requests".to_string(), format_num(total)),
        ("Duration".to_string(), format_duration(elapsed)),
        (
            "Throughput (vectors/s)".to_string(),
            format_num(throughput as u64),
        ),
    ];
    print_markdown_report("Ingest", &metrics);

    Ok(())
}

async fn run_search(
    client: FerresDbClient,
    url: &str,
    http_client: &reqwest::Client,
    duration: Duration,
    concurrency: usize,
    collection: &str,
    dim: u32,
    openai_api_key: Option<&str>,
    embedding_model: &str,
    num_queries: usize,
    limit: usize,
    warmup: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("FerresDB Search Benchmark");
    println!("  target: {}", url);
    println!("  collection: {}", collection);
    println!("  concurrency: {}, duration: {:?}", concurrency, duration);
    if openai_api_key.is_some() {
        println!("  query embeddings: OpenAI ({})", embedding_model);
    } else {
        println!("  query vectors: random (dim={})", dim);
    }

    // No shared histogram lock — each worker accumulates locally and merges at the end.

    // ---------- Phase 1: Pre-compute query pool ----------
    let (query_pool, query_pool_desc) = if let Some(key) = openai_api_key {
        println!("\n[Phase 1] Pre-computing query embeddings via OpenAI...");
        println!("  Provider: OpenAI ({})", embedding_model);
        let phase1_start = Instant::now();
        let texts: Vec<String> = (0..num_queries)
            .map(|i| format!("Query about topic {} for benchmark search.", i))
            .collect();
        let mut all_vectors = Vec::with_capacity(num_queries);
        for chunk in texts.chunks(100) {
            let chunk_vec: Vec<String> = chunk.to_vec();
            let vecs = openai_embed_batch(&chunk_vec, key, embedding_model, http_client).await?;
            all_vectors.extend(vecs);
        }
        let phase1_elapsed = phase1_start.elapsed();
        println!(
            "  Queries: {} embeddings generated in {:.1}s\n",
            all_vectors.len(),
            phase1_elapsed.as_secs_f64()
        );
        let desc = format!("{} vectors (OpenAI {})", all_vectors.len(), embedding_model);
        (all_vectors, desc)
    } else {
        println!(
            "\n[Phase 1] Generating {} random query vectors...\n",
            num_queries
        );
        let mut rng = rand::rngs::StdRng::from_entropy();
        let pool: Vec<Vec<f32>> = (0..num_queries)
            .map(|_| random_vector(dim, &mut rng))
            .collect();
        let desc = format!("{} random vectors (dim={})", num_queries, dim);
        (pool, desc)
    };

    let query_pool = Arc::new(query_pool);
    let pool_size = query_pool.len();

    // ---------- Warmup ----------
    if warmup > 0 {
        for i in 0..warmup {
            let v = &query_pool[i % pool_size];
            let _ = client.search(collection, v, limit, None).await;
        }
    }

    // ---------- Phase 2: Unified benchmark loop ----------
    println!("[Phase 2] Running search benchmark...");
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let start = Instant::now();
    let deadline = start + duration;
    let query_idx = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let client = client.clone();
        let coll = collection.to_string();
        let query_pool = query_pool.clone();
        let query_idx = query_idx.clone();
        let barrier = barrier.clone();
        let h: tokio::task::JoinHandle<(Histogram<u64>, u64, u64)> = tokio::spawn(async move {
            let mut local_hist = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
            let mut local_count: u64 = 0;
            let mut local_attempted: u64 = 0;
            barrier.wait().await;
            while Instant::now() < deadline {
                let idx = query_idx.fetch_add(1, Ordering::Relaxed) as usize % pool_size;
                let vector = &query_pool[idx];
                local_attempted += 1;
                let t0 = Instant::now();
                let res = client.search(&coll, vector, limit, None).await;
                let elapsed_ns = t0.elapsed().as_nanos() as u64;
                if res.is_ok() {
                    local_count += 1;
                    let _ = local_hist.record(elapsed_ns);
                }
            }
            (local_hist, local_count, local_attempted)
        });
        handles.push(h);
    }
    barrier.wait().await;

    let mut hist = Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).unwrap();
    let mut total: u64 = 0;
    let mut attempted: u64 = 0;
    for h in handles {
        if let Ok((worker_hist, worker_count, worker_attempted)) = h.await {
            hist.add(&worker_hist).unwrap();
            total += worker_count;
            attempted += worker_attempted;
        }
    }
    let elapsed = start.elapsed();
    let qps = if elapsed.as_secs_f64() > 0.0 {
        total as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let success_pct = if attempted > 0 {
        100.0 * total as f64 / attempted as f64
    } else {
        0.0
    };

    let (lat_min, lat_avg, lat_p50, lat_p90, lat_p95, lat_p99, lat_max) = {
        let to_ms = |ns: u64| ns as f64 / 1_000_000.0;
        let (min, max) = if total > 0 {
            (hist.min(), hist.max())
        } else {
            (0, 0)
        };
        let avg = if total > 0 { hist.mean() } else { 0.0 };
        let p50 = if total > 0 {
            hist.value_at_quantile(0.50)
        } else {
            0
        };
        let p90 = if total > 0 {
            hist.value_at_quantile(0.90)
        } else {
            0
        };
        let p95 = if total > 0 {
            hist.value_at_quantile(0.95)
        } else {
            0
        };
        let p99 = if total > 0 {
            hist.value_at_quantile(0.99)
        } else {
            0
        };
        (
            to_ms(min),
            avg / 1_000_000.0,
            to_ms(p50),
            to_ms(p90),
            to_ms(p95),
            to_ms(p99),
            to_ms(max),
        )
    };

    println!("\nSearch complete.");
    println!("  Query Pool: {}", query_pool_desc);
    println!("  Total Requests: {}", format_num(attempted));
    println!("  Successful: {} ({:.1}%)", format_num(total), success_pct);
    println!("  Duration: {}", format_duration(elapsed));
    println!("  QPS: {}", format_num(qps as u64));
    println!("  Latency Min: {:.2}ms", lat_min);
    println!("  Latency Avg: {:.2}ms", lat_avg);
    println!("  Latency P50: {:.2}ms", lat_p50);
    println!("  Latency P90: {:.2}ms", lat_p90);
    println!("  Latency P95: {:.2}ms", lat_p95);
    println!("  Latency P99: {:.2}ms", lat_p99);
    println!("  Latency Max: {:.2}ms", lat_max);

    let metrics = vec![
        ("Mode".to_string(), "Search (Read Stress)".to_string()),
        ("Query Pool".to_string(), query_pool_desc.clone()),
        ("Total Requests".to_string(), format_num(attempted)),
        (
            "Successful".to_string(),
            format!("{} ({:.1}%)", format_num(total), success_pct),
        ),
        ("Duration".to_string(), format_duration(elapsed)),
        ("QPS".to_string(), format_num(qps as u64)),
        ("Latency Min".to_string(), format!("{:.2}ms", lat_min)),
        ("Latency Avg".to_string(), format!("{:.2}ms", lat_avg)),
        ("Latency P50".to_string(), format!("{:.2}ms", lat_p50)),
        ("Latency P90".to_string(), format!("{:.2}ms", lat_p90)),
        ("Latency P95".to_string(), format!("{:.2}ms", lat_p95)),
        ("Latency P99".to_string(), format!("{:.2}ms", lat_p99)),
        ("Latency Max".to_string(), format!("{:.2}ms", lat_max)),
    ];
    print_markdown_report("Search", &metrics);

    Ok(())
}

async fn run_chaos(
    client: FerresDbClient,
    duration: Duration,
    writers: usize,
    readers: usize,
    dim: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("FerresDB Chaos Benchmark");
    println!(
        "  duration: {:?}, writers: {}, readers: {}",
        duration, writers, readers
    );

    let base_name = "bench_chaos";
    match client
        .create_collection(base_name, dim, "Cosine", false)
        .await
    {
        Ok(_) => println!("  Collection '{}' created.", base_name),
        Err(e) => {
            let msg = e.to_string();
            let already_exists = msg.to_lowercase().contains("already exists")
                || msg.contains("409")
                || msg.to_lowercase().contains("collection_already_exists");
            if !already_exists {
                return Err(format!("Failed to create collection '{}': {}", base_name, e).into());
            }
            println!("  Collection '{}' already exists, using it.", base_name);
        }
    }

    let writes = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let collections_created = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(writers + readers + 3));
    let deadline = Instant::now() + duration;

    let mp = MultiProgress::new();
    let pb = mp.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("[{elapsed_precise}] chaos {msg}")
            .unwrap(),
    );

    let mut handles = Vec::new();

    for _ in 0..writers {
        let client = client.clone();
        let writes = writes.clone();
        let barrier = barrier.clone();
        let coll = base_name.to_string();
        let h = tokio::spawn(async move {
            barrier.wait().await;
            let mut rng = rand::rngs::StdRng::from_entropy();
            let mut counter = 0u64;
            while Instant::now() < deadline {
                let points: Vec<PointInput> = (0..BATCH_SIZE.min(100))
                    .map(|j| {
                        let id = format!("chaos-{}-{}", counter, j);
                        let vector = random_vector(dim, &mut rng);
                        PointInput {
                            id,
                            vector,
                            metadata: None,
                        }
                    })
                    .collect();
                if client.upsert_points(&coll, &points).await.is_ok() {
                    writes.fetch_add(points.len() as u64, Ordering::Relaxed);
                }
                counter += 1;
            }
        });
        handles.push(h);
    }

    for _ in 0..readers {
        let client = client.clone();
        let reads = reads.clone();
        let barrier = barrier.clone();
        let coll = base_name.to_string();
        let h = tokio::spawn(async move {
            barrier.wait().await;
            let mut rng = rand::rngs::StdRng::from_entropy();
            while Instant::now() < deadline {
                let vector = random_vector(dim, &mut rng);
                if client.search(&coll, &vector, 5, None).await.is_ok() {
                    reads.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        handles.push(h);
    }

    let client_creator = client.clone();
    let barrier_for_creator = barrier.clone();
    let collections_created_creator = collections_created.clone();
    let h_creator = tokio::spawn(async move {
        barrier_for_creator.wait().await;
        let mut rng = rand::rngs::StdRng::from_entropy();
        let mut i = 0u32;
        while Instant::now() < deadline {
            let name = format!("{}_extra_{}", base_name, i);
            if client_creator
                .create_collection(&name, dim, "Cosine", false)
                .await
                .is_ok()
            {
                collections_created_creator.fetch_add(1, Ordering::Relaxed);
            }
            i += 1;
            tokio::time::sleep(Duration::from_millis(rng.gen_range(50..200))).await;
        }
    });
    handles.push(h_creator);

    barrier.wait().await;
    for h in handles {
        let _ = h.await;
    }
    let elapsed = duration;
    pb.finish_with_message("done");

    let w = writes.load(Ordering::Relaxed);
    let r = reads.load(Ordering::Relaxed);
    let c = collections_created.load(Ordering::Relaxed);

    println!("\nChaos complete.");
    println!("  Writes (points): {}", format_num(w));
    println!("  Reads: {}", format_num(r));
    println!("  Collections created: {}", c);
    println!("  Duration: {}", format_duration(elapsed));

    let metrics = vec![
        ("Mode".to_string(), "Chaos (Mixed)".to_string()),
        ("Duration".to_string(), format_duration(elapsed)),
        ("Total Writes (points)".to_string(), format_num(w)),
        ("Total Reads".to_string(), format_num(r)),
        ("Collections Created".to_string(), c.to_string()),
    ];
    print_markdown_report("Chaos", &metrics);

    Ok(())
}
