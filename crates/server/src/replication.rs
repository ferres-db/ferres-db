//! # Replication — worker que consome WAL do líder via gRPC e aplica no VectorDB local.
//!
//! Ativo apenas quando o servidor é iniciado com `--replica-of <ADDR>` (ou `FERRESDB_REPLICA_OF`).
//! Requer feature `grpc`.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, FileStorage, Point};

use crate::grpc::pb::{
    ferres_db_client::FerresDbClient, GetCollectionRequest, ListCollectionsRequest, StreamWalRequest,
    WalDelete, WalEntryMessage, WalUpsert,
};
use crate::state::AppState;

/// Intervalo entre ciclos de replicação quando não há novas entradas.
const REPLICATION_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Executa o worker de replicação: conecta ao líder, lista coleções, aplica WAL em cada uma.
pub async fn run_replication_worker(state: Arc<AppState>) {
    let master_addr = match state.config.replica_of.as_ref() {
        Some(addr) => addr.clone(),
        None => return,
    };

    info!(master = %master_addr, "replication worker starting");

    loop {
        if state.is_shutting_down() {
            info!("replication worker shutting down");
            break;
        }

        let endpoint = match format!("http://{}", master_addr).parse::<tonic::transport::Endpoint>() {
            Ok(ep) => ep,
            Err(e) => {
                error!(error = %e, "invalid replica-of address");
                break;
            }
        };

        match endpoint.connect().await {
            Ok(channel) => {
                let mut client = FerresDbClient::new(channel);
                if let Err(e) = replicate_cycle(&state, &mut client).await {
                    warn!(error = %e, "replication cycle error");
                }
            }
            Err(e) => {
                debug!(error = %e, "could not connect to leader, will retry");
            }
        }

        sleep(REPLICATION_POLL_INTERVAL).await;
    }
}

/// Um ciclo: lista coleções no líder, para cada uma faz stream do WAL e aplica localmente.
async fn replicate_cycle(
    state: &AppState,
    client: &mut FerresDbClient<tonic::transport::Channel>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let list = client
        .list_collections(ListCollectionsRequest {})
        .await?
        .into_inner();

    for info in list.collections {
        let name = info.name;
        if let Err(e) = replicate_collection(state, client, &name).await {
            warn!(collection = %name, error = %e, "replicate collection failed");
        }
    }

    Ok(())
}

/// Garante que a coleção existe localmente (cria a partir do líder se necessário), depois aplica WAL.
async fn replicate_collection(
    state: &AppState,
    client: &mut FerresDbClient<tonic::transport::Channel>,
    collection_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Se a coleção não existe localmente, não temos como obter config do líder via gRPC GetCollection
    // sem ter um client que retorne CollectionConfig. O proto GetCollection retorna dimension/distance.
    if !state.collections.contains_key(collection_name) {
        // Opção: criar coleção vazia com dimension/distance do líder via GetCollection
        let get_resp = client
            .get_collection(tonic::Request::new(GetCollectionRequest {
                name: collection_name.to_string(),
            }))
            .await;
        let get_resp = match get_resp {
            Ok(r) => r.into_inner(),
            Err(e) => {
                debug!(collection = %collection_name, "leader returned error (collection may not exist): {}", e);
                return Ok(());
            }
        };

        let distance = match get_resp.distance {
            1 => DistanceMetric::Cosine,
            2 => DistanceMetric::DotProduct,
            3 => DistanceMetric::Euclidean,
            _ => DistanceMetric::Euclidean,
        };
        let config = CollectionConfig {
            name: collection_name.to_string(),
            dimension: get_resp.dimension as usize,
            distance,
            hnsw: Default::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: Default::default(),
            tiered_storage: Default::default(),
        };
        let collection = Collection::new(config.clone());
        let collection_dir = state
            .config
            .storage_path
            .join("collections")
            .join(collection_name);
        std::fs::create_dir_all(&collection_dir)?;
        FileStorage::save_collection(
            &collection,
            &collection_dir,
            state.config.binary_snapshot,
            state.config.namespace_physical_isolation,
        )?;
        state
            .collections
            .insert(collection_name.to_string(), Arc::new(std::sync::RwLock::new(collection)));
        state
            .query_stats
            .insert(collection_name.to_string(), crate::state::QueryStats::new());
        crate::metrics::COLLECTIONS_ACTIVE.set(state.collections.len() as f64);
        info!(collection = %collection_name, "created collection from leader");
    }

    let from_position = 0u64; // TODO: persistir last position por coleção para incremental
    let request = tonic::Request::new(StreamWalRequest {
        collection_name: collection_name.to_string(),
        from_position,
    });
    let mut stream = client.stream_wal(request).await?.into_inner();

    while let Some(msg) = stream.message().await? {
        for entry in msg.entries {
            apply_wal_entry(state, collection_name, &entry)?;
        }
    }

    Ok(())
}

/// Converte e aplica uma entrada WAL (proto) na coleção local.
fn apply_wal_entry(
    state: &AppState,
    collection_name: &str,
    entry: &WalEntryMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(operation) = &entry.operation else { return Ok(()); };

    let coll_arc = state
        .collections
        .get(collection_name)
        .ok_or_else(|| format!("collection '{}' not found", collection_name))?;

    use crate::grpc::pb::wal_entry_message::Operation;
    match operation {
        Operation::Upsert(upsert) => {
            let metadata = if upsert.metadata_json.is_empty() || upsert.metadata_json == "null" {
                serde_json::Value::Null
            } else {
                serde_json::from_str(&upsert.metadata_json).unwrap_or(serde_json::Value::Null)
            };
            let point = Point::new(
                upsert.id.clone(),
                upsert.vector.clone(),
                metadata,
            ).map_err(|e| format!("invalid point: {}", e))?;
            let mut point = point;
            point.namespace = upsert.namespace.clone();
            point.created_at = upsert.created_at;

            let mut coll = coll_arc.write().map_err(|e| e.to_string())?;
            coll.insert(point).map_err(|e| e.to_string())?;
            coll.mark_dirty();
        }
        Operation::Delete(WalDelete { id }) => {
            let mut coll = coll_arc.write().map_err(|e| e.to_string())?;
            let _ = coll.remove(id);
            coll.mark_dirty();
        }
    }

    Ok(())
}
