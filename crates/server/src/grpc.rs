//! # gRPC service — FerresDB native gRPC API
//!
//! Implementação do serviço gRPC definido em `proto/ferresdb.proto`.
//! Reutiliza toda a lógica do core (`Collection`, `Point`, `MetadataFilter`)
//! e o estado compartilhado (`AppState`) — nenhuma lógica de negócio é duplicada.
//!
//! Ativado apenas com a feature `grpc`.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::info;

use ferres_db_core::{
    Collection, CollectionConfig, FileStorage, MetadataFilter, Point,
};

use crate::state::AppState;

/// Módulo gerado pelo tonic-build a partir de `proto/ferresdb.proto`.
pub mod pb {
    tonic::include_proto!("ferresdb.v1");
}

use pb::ferres_db_server::{FerresDb, FerresDbServer};
use pb::*;

// ─── Helper: converter DistanceMetric proto ↔ core ──────────────────────

fn proto_distance_to_core(d: i32) -> Result<ferres_db_core::DistanceMetric, Status> {
    match d {
        1 => Ok(ferres_db_core::DistanceMetric::Cosine),
        2 => Ok(ferres_db_core::DistanceMetric::DotProduct),
        3 => Ok(ferres_db_core::DistanceMetric::Euclidean),
        _ => Err(Status::invalid_argument("invalid or unspecified distance metric")),
    }
}

fn core_distance_to_proto(d: ferres_db_core::DistanceMetric) -> i32 {
    match d {
        ferres_db_core::DistanceMetric::Cosine => 1,
        ferres_db_core::DistanceMetric::DotProduct => 2,
        ferres_db_core::DistanceMetric::Euclidean => 3,
    }
}

// ─── Service struct ─────────────────────────────────────────────────────

/// Implementação do serviço gRPC do FerresDB.
///
/// Mantém uma referência ao `AppState` compartilhado com o servidor REST,
/// garantindo que ambas as APIs operem sobre os mesmos dados.
pub struct FerresGrpcService {
    state: AppState,
}

impl FerresGrpcService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Cria o `tonic::transport::Server` service pronto para ser adicionado a um router.
    pub fn into_server(self) -> FerresDbServer<Self> {
        FerresDbServer::new(self)
    }
}

// ─── Tipo auxiliar para streaming ───────────────────────────────────────

type GrpcStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

// ─── Implementação do trait gerado ──────────────────────────────────────

#[tonic::async_trait]
impl FerresDb for FerresGrpcService {
    // ── Collections ────────────────────────────────────────────────────

    async fn create_collection(
        &self,
        request: Request<CreateCollectionRequest>,
    ) -> Result<Response<CreateCollectionResponse>, Status> {
        let req = request.into_inner();

        if req.name.is_empty() {
            return Err(Status::invalid_argument("name cannot be empty"));
        }
        if req.dimension == 0 || req.dimension > 4096 {
            return Err(Status::invalid_argument("dimension must be between 1 and 4096"));
        }

        let distance = proto_distance_to_core(req.distance)?;

        // Verifica se já existe
        if self.state.collections.contains_key(&req.name) {
            return Err(Status::already_exists(format!(
                "collection '{}' already exists",
                req.name
            )));
        }

        let bm25_text_field = if req.bm25_text_field.is_empty() {
            "text".to_string()
        } else {
            req.bm25_text_field
        };

        let config = CollectionConfig {
            name: req.name.clone(),
            dimension: req.dimension as usize,
            distance,
            hnsw: Default::default(),
            search_cache_size: 0,
            enable_bm25: req.enable_bm25,
            bm25_text_field,
            quantization: Default::default(),
            tiered_storage: Default::default(),
        };

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let collection = Collection::new(config.clone());
        let collection_arc = Arc::new(std::sync::RwLock::new(collection));

        self.state
            .collections
            .insert(req.name.clone(), collection_arc.clone());
        self.state
            .query_stats
            .insert(req.name.clone(), crate::state::QueryStats::new());
        crate::metrics::COLLECTIONS_ACTIVE.set(self.state.collections.len() as f64);

        // Salva no disco
        let collection_dir = self
            .state
            .config
            .storage_path
            .join("collections")
            .join(&req.name);
        {
            let coll = collection_arc
                .read()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;
            FileStorage::save_collection(&coll, &collection_dir)
                .map_err(|e| Status::internal(e.to_string()))?;
            coll.mark_clean();
        }

        info!(collection = %req.name, "gRPC: collection created");

        Ok(Response::new(CreateCollectionResponse {
            name: config.name,
            dimension: config.dimension as u32,
            distance: core_distance_to_proto(config.distance),
            created_at,
        }))
    }

    async fn get_collection(
        &self,
        request: Request<GetCollectionRequest>,
    ) -> Result<Response<GetCollectionResponse>, Status> {
        let req = request.into_inner();

        let collection_arc = self
            .state
            .collections
            .get(&req.name)
            .ok_or_else(|| Status::not_found(format!("collection '{}' not found", req.name)))?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        let config = coll.config();
        let num_points = coll.len();

        let last_updated = coll
            .points_owned()
            .iter()
            .map(|p| p.created_at)
            .max()
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });

        let index_size_bytes = num_points * config.dimension * 4;

        Ok(Response::new(GetCollectionResponse {
            name: req.name,
            dimension: config.dimension as u32,
            num_points: num_points as u64,
            last_updated,
            distance: core_distance_to_proto(config.distance),
            index_size_bytes: index_size_bytes as u64,
        }))
    }

    async fn list_collections(
        &self,
        _request: Request<ListCollectionsRequest>,
    ) -> Result<Response<ListCollectionsResponse>, Status> {
        let mut collections = Vec::new();

        for entry in self.state.collections.iter() {
            let name = entry.key().clone();
            let coll = entry
                .value()
                .read()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;

            let config = coll.config();
            let num_points = coll.len();

            let created_at = coll
                .points_owned()
                .iter()
                .map(|p| p.created_at)
                .min()
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                });

            collections.push(CollectionInfo {
                name,
                dimension: config.dimension as u32,
                num_points: num_points as u64,
                created_at,
                distance: core_distance_to_proto(config.distance),
            });
        }

        Ok(Response::new(ListCollectionsResponse { collections }))
    }

    async fn delete_collection(
        &self,
        request: Request<DeleteCollectionRequest>,
    ) -> Result<Response<DeleteCollectionResponse>, Status> {
        let req = request.into_inner();

        self.state
            .collections
            .remove(&req.name)
            .ok_or_else(|| Status::not_found(format!("collection '{}' not found", req.name)))?;

        self.state.query_stats.remove(&req.name);
        crate::metrics::COLLECTIONS_ACTIVE.set(self.state.collections.len() as f64);

        // Remove do disco
        let collection_dir = self
            .state
            .config
            .storage_path
            .join("collections")
            .join(&req.name);
        if collection_dir.exists() {
            std::fs::remove_dir_all(&collection_dir)
                .map_err(|e| Status::internal(format!("failed to remove directory: {e}")))?;
        }

        info!(collection = %req.name, "gRPC: collection deleted");
        Ok(Response::new(DeleteCollectionResponse {}))
    }

    // ── Points ─────────────────────────────────────────────────────────

    async fn upsert_points(
        &self,
        request: Request<UpsertPointsRequest>,
    ) -> Result<Response<UpsertPointsResponse>, Status> {
        let req = request.into_inner();

        if req.points.is_empty() {
            return Err(Status::invalid_argument("points must contain at least 1 item"));
        }

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let (upserted, failed) = {
            let mut coll = collection_arc
                .write()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;

            let mut points = Vec::new();
            let mut failed = Vec::new();

            for input in &req.points {
                if let Err(e) = coll.validate_dimension(&input.vector) {
                    failed.push(FailedPoint {
                        id: input.id.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }

                if input.vector.is_empty() {
                    failed.push(FailedPoint {
                        id: input.id.clone(),
                        reason: "vector cannot be empty".to_string(),
                    });
                    continue;
                }

                let metadata: serde_json::Value = if input.metadata_json.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(&input.metadata_json).map_err(|e| {
                        Status::invalid_argument(format!(
                            "invalid metadata JSON for point '{}': {e}",
                            input.id
                        ))
                    })?
                };

                match Point::new(input.id.clone(), input.vector.clone(), metadata) {
                    Ok(point) => points.push(point),
                    Err(e) => {
                        failed.push(FailedPoint {
                            id: input.id.clone(),
                            reason: e.to_string(),
                        });
                    }
                }
            }

            if points.is_empty() {
                return Ok(Response::new(UpsertPointsResponse {
                    upserted: 0,
                    failed,
                }));
            }

            let upserted = match coll.insert_batch(points) {
                Ok(result) => result.inserted,
                Err(err) => {
                    failed.push(FailedPoint {
                        id: "batch".to_string(),
                        reason: err.to_string(),
                    });
                    0
                }
            };
            coll.mark_dirty();
            (upserted, failed)
        };

        Ok(Response::new(UpsertPointsResponse {
            upserted: upserted as u64,
            failed,
        }))
    }

    async fn delete_points(
        &self,
        request: Request<DeletePointsRequest>,
    ) -> Result<Response<DeletePointsResponse>, Status> {
        let req = request.into_inner();

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let deleted = {
            let mut coll = collection_arc
                .write()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;
            coll.delete_points_batch(&req.ids)
                .map_err(|e| Status::internal(e.to_string()))?
        };

        Ok(Response::new(DeletePointsResponse {
            deleted: deleted as u64,
        }))
    }

    async fn get_point(
        &self,
        request: Request<GetPointRequest>,
    ) -> Result<Response<GetPointResponse>, Status> {
        let req = request.into_inner();

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        let point = coll
            .get(&req.id)
            .ok_or_else(|| Status::not_found(format!("point '{}' not found", req.id)))?;

        let metadata_json = serde_json::to_string(&point.metadata)
            .unwrap_or_else(|_| "null".to_string());

        Ok(Response::new(GetPointResponse {
            id: point.id.clone(),
            vector: point.vector.clone(),
            metadata_json,
            created_at: point.created_at,
        }))
    }

    async fn list_points(
        &self,
        request: Request<ListPointsRequest>,
    ) -> Result<Response<ListPointsResponse>, Status> {
        let req = request.into_inner();
        let limit = (req.limit as usize).min(1000).max(1);
        let offset = req.offset as usize;

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        let mut all_points: Vec<GetPointResponse> = coll
            .points_owned()
            .iter()
            .map(|p| GetPointResponse {
                id: p.id.clone(),
                vector: p.vector.clone(),
                metadata_json: serde_json::to_string(&p.metadata)
                    .unwrap_or_else(|_| "null".to_string()),
                created_at: p.created_at,
            })
            .collect();

        // Aplica filtro de metadata se fornecido
        if !req.filter_json.is_empty() {
            let filter_value: serde_json::Value =
                serde_json::from_str(&req.filter_json).map_err(|e| {
                    Status::invalid_argument(format!("invalid filter JSON: {e}"))
                })?;
            let filter = MetadataFilter::from_json(filter_value)
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            all_points.retain(|p| {
                let meta: serde_json::Value =
                    serde_json::from_str(&p.metadata_json).unwrap_or(serde_json::Value::Null);
                filter.matches(&meta)
            });
        }

        let total = all_points.len();
        let has_more = offset + limit < total;

        let points: Vec<GetPointResponse> =
            all_points.into_iter().skip(offset).take(limit).collect();

        Ok(Response::new(ListPointsResponse {
            points,
            total: total as u64,
            limit: limit as u64,
            offset: offset as u64,
            has_more,
        }))
    }

    // ── Search ─────────────────────────────────────────────────────────

    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let req = request.into_inner();
        let start = Instant::now();

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        coll.validate_dimension(&req.vector)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let raw = coll
            .search(&req.vector, req.limit as usize)
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut results: Vec<SearchResult> = raw
            .into_iter()
            .filter_map(|(id, score)| {
                let point = coll.get(&id)?;
                Some(SearchResult {
                    id,
                    score,
                    metadata_json: serde_json::to_string(&point.metadata)
                        .unwrap_or_else(|_| "null".to_string()),
                })
            })
            .collect();

        // Drop lock antes de filtrar
        drop(coll);
        drop(collection_arc);

        // Aplica filtro
        if !req.filter_json.is_empty() {
            let filter_value: serde_json::Value =
                serde_json::from_str(&req.filter_json).map_err(|e| {
                    Status::invalid_argument(format!("invalid filter JSON: {e}"))
                })?;
            let filter = MetadataFilter::from_json(filter_value)
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            if !filter.is_empty() {
                results.retain(|r| {
                    let meta: serde_json::Value =
                        serde_json::from_str(&r.metadata_json).unwrap_or(serde_json::Value::Null);
                    filter.matches(&meta)
                });
            }
        }

        let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        // Registra métricas
        self.state
            .query_stats
            .entry(req.collection.clone())
            .or_insert_with(|| crate::state::QueryStats::new())
            .record_query(took_ms);
        self.state.global_query_stats.record(&req.collection, took_ms);
        crate::metrics::QUERIES_TOTAL
            .with_label_values(&[&req.collection])
            .inc();
        crate::metrics::QUERY_DURATION_MS
            .with_label_values(&[&req.collection])
            .observe(took_ms as f64);

        Ok(Response::new(SearchResponse { results, took_ms }))
    }

    async fn hybrid_search(
        &self,
        request: Request<HybridSearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let req = request.into_inner();
        let start = Instant::now();

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        coll.validate_dimension(&req.query_vector)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let alpha = if req.alpha == 0.0 && req.fusion.is_empty() {
            0.5
        } else {
            req.alpha
        };

        if !(0.0..=1.0).contains(&alpha) {
            return Err(Status::invalid_argument("alpha must be between 0 and 1"));
        }

        let fusion_strategy = match req.fusion.as_str() {
            "" | "weighted" => ferres_db_core::FusionStrategy::WeightedScore { alpha },
            "rrf" => {
                let k = if req.rrf_k == 0 {
                    ferres_db_core::DEFAULT_RRF_K
                } else {
                    req.rrf_k as usize
                };
                ferres_db_core::FusionStrategy::RRF { k }
            }
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown fusion strategy '{other}': must be 'weighted' or 'rrf'"
                )));
            }
        };

        let hybrid_results = coll
            .hybrid_search(
                &req.query_vector,
                &req.query_text,
                req.limit as usize,
                &fusion_strategy,
            )
            .map_err(|e| Status::internal(e.to_string()))?;

        let results: Vec<SearchResult> = hybrid_results
            .into_iter()
            .filter_map(|(id, score)| {
                let point = coll.get(&id)?;
                Some(SearchResult {
                    id,
                    score,
                    metadata_json: serde_json::to_string(&point.metadata)
                        .unwrap_or_else(|_| "null".to_string()),
                })
            })
            .collect();

        drop(coll);
        drop(collection_arc);

        let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        self.state
            .query_stats
            .entry(req.collection.clone())
            .or_insert_with(|| crate::state::QueryStats::new())
            .record_query(took_ms);
        self.state.global_query_stats.record(&req.collection, took_ms);
        crate::metrics::QUERIES_TOTAL
            .with_label_values(&[&req.collection])
            .inc();
        crate::metrics::QUERY_DURATION_MS
            .with_label_values(&[&req.collection])
            .observe(took_ms as f64);

        Ok(Response::new(SearchResponse { results, took_ms }))
    }

    async fn explain_search(
        &self,
        request: Request<ExplainSearchRequest>,
    ) -> Result<Response<ExplainSearchResponse>, Status> {
        let req = request.into_inner();
        let start = Instant::now();

        let collection_arc = self
            .state
            .collections
            .get(&req.collection)
            .ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        coll.validate_dimension(&req.vector)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let query_vector_norm = req
            .vector
            .iter()
            .map(|x| (*x as f64) * (*x as f64))
            .sum::<f64>()
            .sqrt() as f32;

        let distance_metric = format!("{:?}", coll.config().distance);

        let filter = if !req.filter_json.is_empty() {
            let fv: serde_json::Value = serde_json::from_str(&req.filter_json)
                .map_err(|e| Status::invalid_argument(format!("invalid filter JSON: {e}")))?;
            MetadataFilter::from_json(fv).map_err(|e| Status::invalid_argument(e.to_string()))?
        } else {
            MetadataFilter::empty()
        };

        let search_limit = if filter.is_empty() {
            req.limit as usize
        } else {
            let expanded = (req.limit as usize).saturating_mul(10);
            expanded.min(coll.len().max(req.limit as usize))
        };

        let raw_results = coll
            .search_explain(&req.vector, search_limit)
            .map_err(|e| Status::internal(e.to_string()))?;

        let candidates_scanned = raw_results.len();

        let mut explain_results = Vec::new();
        let mut rank_after_counter = 0u32;
        let mut total_tombstones_skipped = 0u64;

        for (rank_before_idx, (id, score, meta)) in raw_results.iter().enumerate() {
            let point = match coll.get(id) {
                Some(p) => p,
                None => continue,
            };

            total_tombstones_skipped = meta.tombstones_skipped as u64;

            let filter_eval = if !filter.is_empty() {
                let condition_results: Vec<ferres_db_core::ConditionResult> = filter
                    .conditions()
                    .iter()
                    .map(|cond| ferres_db_core::evaluate_condition(cond, &point.metadata))
                    .collect();
                condition_results.iter().all(|c| c.passed)
            } else {
                true
            };

            if filter_eval {
                rank_after_counter += 1;
            }

            let mut score_breakdown = std::collections::HashMap::new();
            score_breakdown.insert("vector_score".to_string(), *score);

            explain_results.push(ExplainResult {
                id: id.clone(),
                score: *score,
                distance_metric: distance_metric.clone(),
                raw_distance: *score,
                score_breakdown,
                rank_before_filter: (rank_before_idx + 1) as u32,
                rank_after_filter: if filter_eval { rank_after_counter } else { 0 },
            });
        }

        let candidates_after_filter = explain_results
            .iter()
            .filter(|r| r.rank_after_filter > 0)
            .count();

        let index_stats = Some(IndexStats {
            total_points: coll.len() as u64,
            hnsw_layers: coll.config().hnsw.max_layer as u32,
            ef_search_used: coll.config().hnsw.ef_search as u32,
            tombstones_skipped: total_tombstones_skipped,
        });

        drop(coll);
        drop(collection_arc);

        let _took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        crate::metrics::QUERIES_TOTAL
            .with_label_values(&[&req.collection])
            .inc();
        crate::metrics::QUERY_DURATION_MS
            .with_label_values(&[&req.collection])
            .observe(_took_ms as f64);

        Ok(Response::new(ExplainSearchResponse {
            query_vector_norm,
            distance_metric,
            candidates_scanned: candidates_scanned as u64,
            candidates_after_filter: candidates_after_filter as u64,
            results: explain_results,
            index_stats,
        }))
    }

    // ── Streaming ──────────────────────────────────────────────────────

    type StreamUpsertStream = GrpcStream<UpsertPointsResponse>;

    async fn stream_upsert(
        &self,
        request: Request<tonic::Streaming<UpsertPointsRequest>>,
    ) -> Result<Response<Self::StreamUpsertStream>, Status> {
        let state = self.state.clone();
        let mut stream = request.into_inner();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                let req = req?;

                if req.points.is_empty() {
                    yield UpsertPointsResponse { upserted: 0, failed: vec![] };
                    continue;
                }

                // Toda a interação com o lock é síncrona (sem .await dentro do bloco)
                let response = do_upsert_sync(&state, &req)?;
                yield response;
            }
        };

        Ok(Response::new(Box::pin(output)))
    }

    type StreamSearchStream = GrpcStream<SearchResponse>;

    async fn stream_search(
        &self,
        request: Request<tonic::Streaming<SearchRequest>>,
    ) -> Result<Response<Self::StreamSearchStream>, Status> {
        let state = self.state.clone();
        let mut stream = request.into_inner();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                let req = req?;

                // Toda a interação com o lock é síncrona (sem .await dentro do bloco)
                let response = do_search_sync(&state, &req)?;
                yield response;
            }
        };

        Ok(Response::new(Box::pin(output)))
    }
}

// ─── Helpers síncronos para streaming (evitam lock across await) ────────

/// Executa upsert sincronamente (sem .await) para que o RwLockWriteGuard
/// não precise cruzar um ponto de suspensão.
fn do_upsert_sync(
    state: &AppState,
    req: &UpsertPointsRequest,
) -> Result<UpsertPointsResponse, Status> {
    let collection_arc = state
        .collections
        .get(&req.collection)
        .ok_or_else(|| Status::not_found(format!("collection '{}' not found", req.collection)))?;

    let mut coll = collection_arc
        .write()
        .map_err(|e| Status::internal(format!("lock error: {e}")))?;

    let mut points = Vec::new();
    let mut failed = Vec::new();

    for input in &req.points {
        if let Err(e) = coll.validate_dimension(&input.vector) {
            failed.push(FailedPoint {
                id: input.id.clone(),
                reason: e.to_string(),
            });
            continue;
        }
        let metadata: serde_json::Value = if input.metadata_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&input.metadata_json).map_err(|e| {
                Status::invalid_argument(format!("invalid metadata: {e}"))
            })?
        };
        match Point::new(input.id.clone(), input.vector.clone(), metadata) {
            Ok(point) => points.push(point),
            Err(e) => failed.push(FailedPoint {
                id: input.id.clone(),
                reason: e.to_string(),
            }),
        }
    }

    if points.is_empty() {
        return Ok(UpsertPointsResponse {
            upserted: 0,
            failed,
        });
    }

    let upserted = match coll.insert_batch(points) {
        Ok(r) => r.inserted,
        Err(e) => {
            failed.push(FailedPoint {
                id: "batch".to_string(),
                reason: e.to_string(),
            });
            0
        }
    };
    coll.mark_dirty();

    Ok(UpsertPointsResponse {
        upserted: upserted as u64,
        failed,
    })
}

/// Executa search sincronamente (sem .await) para que o RwLockReadGuard
/// não precise cruzar um ponto de suspensão.
fn do_search_sync(state: &AppState, req: &SearchRequest) -> Result<SearchResponse, Status> {
    let start = Instant::now();

    let collection_arc = state
        .collections
        .get(&req.collection)
        .ok_or_else(|| Status::not_found(format!("collection '{}' not found", req.collection)))?;

    let coll = collection_arc
        .read()
        .map_err(|e| Status::internal(format!("lock error: {e}")))?;

    coll.validate_dimension(&req.vector)
        .map_err(|e| Status::invalid_argument(e.to_string()))?;

    let raw = coll
        .search(&req.vector, req.limit as usize)
        .map_err(|e| Status::internal(e.to_string()))?;

    let mut results: Vec<SearchResult> = raw
        .into_iter()
        .filter_map(|(id, score)| {
            let point = coll.get(&id)?;
            Some(SearchResult {
                id,
                score,
                metadata_json: serde_json::to_string(&point.metadata)
                    .unwrap_or_else(|_| "null".to_string()),
            })
        })
        .collect();

    // Drop lock antes de filtrar
    drop(coll);
    drop(collection_arc);

    if !req.filter_json.is_empty() {
        let fv: serde_json::Value = serde_json::from_str(&req.filter_json)
            .map_err(|e| Status::invalid_argument(format!("invalid filter: {e}")))?;
        let filter = MetadataFilter::from_json(fv)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        if !filter.is_empty() {
            results.retain(|r| {
                let meta: serde_json::Value =
                    serde_json::from_str(&r.metadata_json).unwrap_or(serde_json::Value::Null);
                filter.matches(&meta)
            });
        }
    }

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

    state
        .query_stats
        .entry(req.collection.clone())
        .or_insert_with(|| crate::state::QueryStats::new())
        .record_query(took_ms);
    state.global_query_stats.record(&req.collection, took_ms);

    Ok(SearchResponse { results, took_ms })
}
