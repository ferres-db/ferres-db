//! # Lock Safety — stress tests
//!
//! Verifica que N writers + M readers concorrentes completam sem deadlock.
//! Se um guard de lock fosse mantido através de um ponto .await, os writers
//! bloqueariam os readers indefinidamente e o timeout de 10s dispararia.

use std::sync::{Arc, RwLock};

use futures::future::join_all;
use tokio::time::{timeout, Duration};

use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, HnswConfig, Point};

const N_WRITERS: usize = 4;
const N_READERS: usize = 8;
const OPS_PER_TASK: usize = 50;
const TIMEOUT_SECS: u64 = 10;

fn make_collection() -> Arc<RwLock<Collection>> {
    let config = CollectionConfig {
        name: "stress".to_string(),
        dimension: 4,
        distance: DistanceMetric::Cosine,
        hnsw: HnswConfig::default(),
        search_cache_size: 0,
        enable_bm25: false,
        bm25_text_field: "text".to_string(),
        quantization: Default::default(),
        tiered_storage: Default::default(),
        retention_days: None,
    };
    Arc::new(RwLock::new(Collection::new(config)))
}

/// N writers and M readers run concurrently.
/// Each task acquires the lock in a scoped block and yields AFTER releasing it.
/// Completes within TIMEOUT_SECS → no deadlock.
#[tokio::test]
async fn test_concurrent_rw_no_deadlock() {
    let collection = make_collection();

    let mut handles = Vec::new();

    for i in 0..N_WRITERS {
        let coll = collection.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..OPS_PER_TASK {
                {
                    let mut c = coll.write().unwrap();
                    let point = Point::new(
                        format!("w{i}-{j}"),
                        vec![0.1_f32, 0.2, 0.3, 0.4],
                        serde_json::json!({"writer": i}),
                    )
                    .unwrap();
                    c.insert_batch(vec![point]).unwrap();
                } // write lock released here
                tokio::task::yield_now().await;
            }
        }));
    }

    for _ in 0..N_READERS {
        let coll = collection.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..OPS_PER_TASK {
                let _len = {
                    let c = coll.read().unwrap();
                    c.len()
                }; // read lock released here
                tokio::task::yield_now().await;
            }
        }));
    }

    timeout(Duration::from_secs(TIMEOUT_SECS), join_all(handles))
        .await
        .unwrap_or_else(|_| panic!("deadlock: tasks did not complete within {TIMEOUT_SECS}s"))
        .into_iter()
        .for_each(|r| r.expect("task panicked"));
}

/// Verifies that readers are not starved: all readers complete while writers
/// are running. Starvation manifests as reader tasks timing out.
#[tokio::test]
async fn test_readers_not_starved_by_writers() {
    let collection = make_collection();

    let mut handles = Vec::new();

    for i in 0..N_WRITERS {
        let coll = collection.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..OPS_PER_TASK {
                {
                    let mut c = coll.write().unwrap();
                    let p = Point::new(
                        format!("sw{i}-{j}"),
                        vec![0.5_f32, 0.5, 0.5, 0.5],
                        serde_json::Value::Null,
                    )
                    .unwrap();
                    c.insert_batch(vec![p]).unwrap();
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    let read_completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for _ in 0..N_READERS {
        let coll = collection.clone();
        let counter = read_completions.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..OPS_PER_TASK {
                let _len = { coll.read().unwrap().len() };
                tokio::task::yield_now().await;
            }
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
    }

    timeout(Duration::from_secs(TIMEOUT_SECS), join_all(handles))
        .await
        .unwrap_or_else(|_| panic!("starvation or deadlock: did not finish within {TIMEOUT_SECS}s"))
        .into_iter()
        .for_each(|r| r.expect("task panicked"));

    assert_eq!(
        read_completions.load(std::sync::atomic::Ordering::Relaxed),
        N_READERS,
        "not all reader tasks completed"
    );
}
