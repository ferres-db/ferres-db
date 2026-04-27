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
    build_search_explanation, Collection, CollectionConfig, FileStorage, MetadataFilter, Point,
    Wal, WalEntry, WalOperation,
};

use crate::state::AppState;
use crate::time::unix_now;

/// Módulo gerado pelo tonic-build a partir de `proto/ferresdb.proto`.
pub mod pb {
    tonic::include_proto!("ferresdb.v1");
}

use pb::ferres_db_server::{FerresDb, FerresDbServer};
use pb::*;

// ─── Helper: WAL entry to proto (replication) ───────────────────────────

fn wal_entry_to_proto(entry: WalEntry) -> WalEntryMessage {
    use pb::wal_entry_message::Operation;
    let operation = match entry.operation {
        WalOperation::Upsert { point } => {
            let metadata_json =
                serde_json::to_string(&point.metadata).unwrap_or_else(|_| "null".to_string());
            Operation::Upsert(WalUpsert {
                id: point.id,
                vector: point.vector,
                metadata_json,
                created_at: point.created_at,
                namespace: point.namespace,
            })
        }
        WalOperation::Delete { id } => Operation::Delete(WalDelete { id }),
        WalOperation::Link { from, to } => Operation::Link(WalLink { from, to }),
    };
    WalEntryMessage {
        timestamp: entry.timestamp,
        operation: Some(operation),
    }
}

// ─── Helper: converter DistanceMetric proto ↔ core ──────────────────────

fn proto_distance_to_core(d: i32) -> Result<ferres_db_core::DistanceMetric, Status> {
    match d {
        1 => Ok(ferres_db_core::DistanceMetric::Cosine),
        2 => Ok(ferres_db_core::DistanceMetric::DotProduct),
        3 => Ok(ferres_db_core::DistanceMetric::Euclidean),
        _ => Err(Status::invalid_argument(
            "invalid or unspecified distance metric",
        )),
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
            return Err(Status::invalid_argument(
                "dimension must be between 1 and 4096",
            ));
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
            retention_days: None,
        };

        let created_at = unix_now();

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
            let binary = self.state.config.binary_snapshot;
            let ns_isolation = self.state.config.namespace_physical_isolation;
            FileStorage::save_collection(&coll, &collection_dir, binary, ns_isolation)
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
            .unwrap_or_else(unix_now);

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
                .unwrap_or_else(unix_now);

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
            return Err(Status::invalid_argument(
                "points must contain at least 1 item",
            ));
        }

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

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
                    Ok(mut point) => {
                        point.namespace = input.namespace.clone();
                        points.push(point);
                    }
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

        if upserted > 0 {
            let now_secs = unix_now();
            self.state.record_ingest(now_secs, upserted as u64);
        }

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

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

        let keys: Vec<String> = req
            .ids
            .iter()
            .map(|id| Point::storage_id_from_parts(req.namespace.as_deref(), id))
            .collect();
        let deleted = {
            let mut coll = collection_arc
                .write()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;
            coll.delete_points_batch(&keys)
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

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        let key = Point::storage_id_from_parts(req.namespace.as_deref(), &req.id);
        let point = coll
            .get(&key)
            .ok_or_else(|| Status::not_found(format!("point '{}' not found", req.id)))?;

        let metadata_json =
            serde_json::to_string(&point.metadata).unwrap_or_else(|_| "null".to_string());

        Ok(Response::new(GetPointResponse {
            id: point.id.clone(),
            vector: point.vector.clone(),
            metadata_json,
            created_at: point.created_at,
            namespace: point.namespace.clone(),
        }))
    }

    async fn list_points(
        &self,
        request: Request<ListPointsRequest>,
    ) -> Result<Response<ListPointsResponse>, Status> {
        let req = request.into_inner();
        let limit = (req.limit as usize).min(1000).max(1);
        let offset = req.offset as usize;

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

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
                namespace: p.namespace.clone(),
            })
            .collect();

        // Aplica filtro de metadata se fornecido
        if !req.filter_json.is_empty() {
            let filter_value: serde_json::Value = serde_json::from_str(&req.filter_json)
                .map_err(|e| Status::invalid_argument(format!("invalid filter JSON: {e}")))?;
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

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

        let coll = collection_arc
            .read()
            .map_err(|e| Status::internal(format!("lock error: {e}")))?;

        coll.validate_dimension(&req.vector)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let mut filter = if req.filter_json.is_empty() {
            MetadataFilter::empty()
        } else {
            let filter_value: serde_json::Value = serde_json::from_str(&req.filter_json)
                .map_err(|e| Status::invalid_argument(format!("invalid filter JSON: {e}")))?;
            MetadataFilter::from_json(filter_value)
                .map_err(|e| Status::invalid_argument(e.to_string()))?
        };
        if let Some(ref ns) = req.namespace {
            filter.namespace = Some(ns.clone());
        }
        let raw = if filter.is_empty() {
            coll.search(&req.vector, req.limit as usize, None, None)
                .map_err(|e| Status::internal(e.to_string()))?
        } else {
            let predicate = |id: &str| {
                coll.get(id)
                    .map(|p| filter.matches_point(&p))
                    .unwrap_or(false)
            };
            coll.search(&req.vector, req.limit as usize, Some(&predicate), None)
                .map_err(|e| Status::internal(e.to_string()))?
        };

        let results: Vec<SearchResult> = raw
            .into_iter()
            .filter_map(|(storage_id, score)| {
                let point = coll.get(&storage_id)?;
                Some(SearchResult {
                    id: point.id.clone(),
                    score,
                    metadata_json: serde_json::to_string(&point.metadata)
                        .unwrap_or_else(|_| "null".to_string()),
                    namespace: point.namespace.clone(),
                })
            })
            .collect();

        drop(coll);
        drop(collection_arc);

        let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        // Registra métricas
        self.state
            .query_stats
            .entry(req.collection.clone())
            .or_insert_with(|| crate::state::QueryStats::new())
            .record_query(took_ms);
        self.state
            .global_query_stats
            .record(&req.collection, took_ms);
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

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

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
            .filter_map(|(storage_id, score)| {
                let point = coll.get(&storage_id)?;
                if let Some(ref ns) = req.namespace {
                    if point.namespace.as_deref() != Some(ns.as_str()) {
                        return None;
                    }
                }
                Some(SearchResult {
                    id: point.id.clone(),
                    score,
                    metadata_json: serde_json::to_string(&point.metadata)
                        .unwrap_or_else(|_| "null".to_string()),
                    namespace: point.namespace.clone(),
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
        self.state
            .global_query_stats
            .record(&req.collection, took_ms);
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

        // Parse do filtro e merge de namespace
        let mut filter = if !req.filter_json.is_empty() {
            let fv: serde_json::Value = serde_json::from_str(&req.filter_json)
                .map_err(|e| Status::invalid_argument(format!("invalid filter JSON: {e}")))?;
            Some(
                MetadataFilter::from_json(fv)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?,
            )
        } else {
            Some(MetadataFilter::empty())
        };
        if let (Some(ref mut f), Some(ref ns)) = (filter.as_mut(), &req.namespace) {
            f.namespace = Some(ns.clone());
        }
        let filter = filter.and_then(|f| if f.is_empty() { None } else { Some(f) });

        let collection_arc = {
            let ref_guard = self.state.collections.get(&req.collection).ok_or_else(|| {
                Status::not_found(format!("collection '{}' not found", req.collection))
            })?;
            Arc::clone(ref_guard.value())
        };

        // Delega ao core: toda a lógica de explain está em build_search_explanation
        let explanation = {
            let coll = collection_arc
                .read()
                .map_err(|e| Status::internal(format!("lock error: {e}")))?;
            build_search_explanation(&coll, &req.vector, req.limit as usize, filter, None)
                .map_err(|e| Status::internal(e.to_string()))?
        };

        let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

        crate::metrics::QUERIES_TOTAL
            .with_label_values(&[&req.collection])
            .inc();
        crate::metrics::QUERY_DURATION_MS
            .with_label_values(&[&req.collection])
            .observe(took_ms as f64);

        // Converte SearchExplanation do core para tipos protobuf
        let results: Vec<ExplainResult> = explanation
            .results
            .iter()
            .map(|r| {
                // Converte FilterExplanation do core → FilterEvaluation proto
                let filter_evaluation = r.filter_evaluation.as_ref().map(|fe| {
                    let conditions = fe
                        .conditions
                        .iter()
                        .map(|c| ConditionResult {
                            field: c.field.clone(),
                            operator: c.operator.clone(),
                            expected_json: serde_json::to_string(&c.expected)
                                .unwrap_or_else(|_| "null".to_string()),
                            actual_json: serde_json::to_string(&c.actual)
                                .unwrap_or_else(|_| "null".to_string()),
                            passed: c.passed,
                        })
                        .collect();
                    FilterEvaluation {
                        conditions,
                        passed: fe.passed,
                    }
                });

                ExplainResult {
                    id: r.id.clone(),
                    score: r.score,
                    distance_metric: r.distance_metric.clone(),
                    raw_distance: r.raw_distance,
                    score_breakdown: r.score_breakdown.clone(),
                    rank_before_filter: r.rank_before_filter as u32,
                    rank_after_filter: r.rank_after_filter as u32,
                    similarity: r.similarity,
                    filter_evaluation,
                }
            })
            .collect();

        let index_stats = Some(IndexStats {
            total_points: explanation.index_stats.total_points as u64,
            hnsw_layers: explanation.index_stats.hnsw_layers as u32,
            ef_search_used: explanation.index_stats.ef_search_used as u32,
            tombstones_skipped: explanation.index_stats.tombstones_skipped as u64,
        });

        Ok(Response::new(ExplainSearchResponse {
            query_vector_norm: explanation.query_vector_norm,
            distance_metric: explanation.distance_metric,
            candidates_scanned: explanation.candidates_scanned as u64,
            candidates_after_filter: explanation.candidates_after_filter as u64,
            results,
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

    // ── Replication: StreamWal ─────────────────────────────────────────

    type StreamWalStream = GrpcStream<StreamWalResponse>;

    async fn stream_wal(
        &self,
        request: Request<StreamWalRequest>,
    ) -> Result<Response<Self::StreamWalStream>, Status> {
        let req = request.into_inner();
        if req.collection_name.is_empty() {
            return Err(Status::invalid_argument("collection_name cannot be empty"));
        }
        let collection_dir = self
            .state
            .config
            .storage_path
            .join("collections")
            .join(&req.collection_name);
        let entries = Wal::stream_from(&collection_dir, req.from_position)
            .map_err(|e| Status::internal(e.to_string()))?;
        let messages: Vec<WalEntryMessage> = entries.into_iter().map(wal_entry_to_proto).collect();
        let stream = tokio_stream::iter([Ok(StreamWalResponse { entries: messages })]);
        Ok(Response::new(Box::pin(stream)))
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
            serde_json::from_str(&input.metadata_json)
                .map_err(|e| Status::invalid_argument(format!("invalid metadata: {e}")))?
        };
        match Point::new(input.id.clone(), input.vector.clone(), metadata) {
            Ok(mut point) => {
                if let Some(ttl_seconds) = input.ttl_seconds {
                    let now_secs = unix_now();
                    point.expires_at = Some(now_secs.saturating_add(ttl_seconds));
                }
                points.push(point);
            }
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

    if upserted > 0 {
        let now_secs = unix_now();
        state.record_ingest(now_secs, upserted as u64);
    }

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

    let mut filter = if req.filter_json.is_empty() {
        MetadataFilter::empty()
    } else {
        let fv: serde_json::Value = serde_json::from_str(&req.filter_json)
            .map_err(|e| Status::invalid_argument(format!("invalid filter: {e}")))?;
        MetadataFilter::from_json(fv).map_err(|e| Status::invalid_argument(e.to_string()))?
    };
    if let Some(ref ns) = req.namespace {
        filter.namespace = Some(ns.clone());
    }
    let raw = if filter.is_empty() {
        coll.search(&req.vector, req.limit as usize, None, None)
            .map_err(|e| Status::internal(e.to_string()))?
    } else {
        let predicate = |id: &str| {
            coll.get(id)
                .map(|p| filter.matches_point(&p))
                .unwrap_or(false)
        };
        coll.search(&req.vector, req.limit as usize, Some(&predicate), None)
            .map_err(|e| Status::internal(e.to_string()))?
    };

    let results: Vec<SearchResult> = raw
        .into_iter()
        .filter_map(|(storage_id, score)| {
            let point = coll.get(&storage_id)?;
            Some(SearchResult {
                id: point.id.clone(),
                score,
                metadata_json: serde_json::to_string(&point.metadata)
                    .unwrap_or_else(|_| "null".to_string()),
                namespace: point.namespace.clone(),
            })
        })
        .collect();

    drop(coll);
    drop(collection_arc);

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

    state
        .query_stats
        .entry(req.collection.clone())
        .or_insert_with(|| crate::state::QueryStats::new())
        .record_query(took_ms);
    state.global_query_stats.record(&req.collection, took_ms);

    Ok(SearchResponse { results, took_ms })
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ServerConfig;
    use serde_json::json;
    use tonic::Request;

    /// Cria um `FerresGrpcService` com AppState temporário para testes.
    fn setup_grpc_service() -> (FerresGrpcService, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            storage_path: temp_dir.path().to_path_buf(),
            log_level: "error".to_string(),
            ..Default::default()
        };
        let state = AppState::new(config, None, None, None, None).unwrap();
        let service = FerresGrpcService::new(state);
        (service, temp_dir)
    }

    /// Helper: cria uma coleção via gRPC.
    async fn create_test_collection(service: &FerresGrpcService, name: &str, dimension: u32) {
        service
            .create_collection(Request::new(CreateCollectionRequest {
                name: name.to_string(),
                dimension,
                distance: 3, // Euclidean
                enable_bm25: false,
                bm25_text_field: String::new(),
            }))
            .await
            .unwrap();
    }

    /// Helper: insere pontos via gRPC.
    async fn upsert_test_points(
        service: &FerresGrpcService,
        collection: &str,
        points: Vec<PointInput>,
    ) {
        service
            .upsert_points(Request::new(UpsertPointsRequest {
                collection: collection.to_string(),
                points,
            }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_explain_search_filter_evaluation_present() {
        let (service, _temp) = setup_grpc_service();
        let coll_name = "grpc_explain_test";

        // Cria coleção com 3 dimensões, Euclidean
        create_test_collection(&service, coll_name, 3).await;

        // Insere pontos com metadata variada
        let points = vec![
            PointInput {
                id: "p1".to_string(),
                vector: vec![1.0, 0.0, 0.0],
                metadata_json: json!({"category": "tech", "price": 50}).to_string(),
                ..Default::default()
            },
            PointInput {
                id: "p2".to_string(),
                vector: vec![0.0, 1.0, 0.0],
                metadata_json: json!({"category": "science", "price": 200}).to_string(),
                ..Default::default()
            },
            PointInput {
                id: "p3".to_string(),
                vector: vec![0.9, 0.1, 0.0],
                metadata_json: json!({"category": "tech", "price": 30}).to_string(),
                ..Default::default()
            },
        ];
        upsert_test_points(&service, coll_name, points).await;

        // Faz explain_search COM filtro
        let filter = json!({
            "category": "tech",
            "price": { "$lte": 100 }
        });
        let response = service
            .explain_search(Request::new(ExplainSearchRequest {
                collection: coll_name.to_string(),
                vector: vec![1.0, 0.0, 0.0],
                limit: 10,
                filter_json: filter.to_string(),
                ..Default::default()
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.results.is_empty(), "deve retornar resultados");

        // Verifica que TODOS os resultados têm filter_evaluation
        for result in &resp.results {
            let fe = result
                .filter_evaluation
                .as_ref()
                .expect("filter_evaluation deve estar presente quando filtro é aplicado");

            // Deve ter 2 condições (category + price)
            assert_eq!(
                fe.conditions.len(),
                2,
                "deve ter 2 condições para resultado '{}'",
                result.id
            );

            // Cada condição deve ter campos preenchidos
            for cond in &fe.conditions {
                assert!(!cond.field.is_empty(), "field não deve ser vazio");
                assert!(!cond.operator.is_empty(), "operator não deve ser vazio");
                assert!(
                    !cond.expected_json.is_empty(),
                    "expected_json não deve ser vazio"
                );
                assert!(
                    !cond.actual_json.is_empty(),
                    "actual_json não deve ser vazio"
                );
            }
        }

        // Verifica p2 (science, price=200): não deve passar no filtro
        let p2 = resp.results.iter().find(|r| r.id == "p2");
        if let Some(p2) = p2 {
            let fe = p2.filter_evaluation.as_ref().unwrap();
            assert!(!fe.passed, "p2 não deveria passar no filtro");
            assert_eq!(
                p2.rank_after_filter, 0,
                "rank_after_filter deve ser 0 para p2"
            );

            // A condição category=$eq deve falhar (science != tech)
            let cat_cond = fe
                .conditions
                .iter()
                .find(|c| c.field == "category")
                .unwrap();
            assert_eq!(cat_cond.operator, "$eq");
            assert!(
                !cat_cond.passed,
                "condição category=$eq deve falhar para p2"
            );
            assert_eq!(cat_cond.expected_json, "\"tech\"");
            assert_eq!(cat_cond.actual_json, "\"science\"");
        }

        // Verifica p1 (tech, price=50): deve passar no filtro
        let p1 = resp.results.iter().find(|r| r.id == "p1");
        if let Some(p1) = p1 {
            let fe = p1.filter_evaluation.as_ref().unwrap();
            assert!(fe.passed, "p1 deveria passar no filtro");
            assert!(
                p1.rank_after_filter > 0,
                "rank_after_filter deve ser > 0 para p1"
            );

            // Todas as condições devem passar
            for cond in &fe.conditions {
                assert!(
                    cond.passed,
                    "condição {}={} deve passar para p1",
                    cond.field, cond.operator
                );
            }
        }

        // Verifica p3 (tech, price=30): deve passar no filtro
        let p3 = resp.results.iter().find(|r| r.id == "p3");
        if let Some(p3) = p3 {
            let fe = p3.filter_evaluation.as_ref().unwrap();
            assert!(fe.passed, "p3 deveria passar no filtro");
            assert!(
                p3.rank_after_filter > 0,
                "rank_after_filter deve ser > 0 para p3"
            );
        }
    }

    #[tokio::test]
    async fn test_explain_search_no_filter_no_evaluation() {
        let (service, _temp) = setup_grpc_service();
        let coll_name = "grpc_explain_no_filter";

        create_test_collection(&service, coll_name, 3).await;

        let points = vec![PointInput {
            id: "p1".to_string(),
            vector: vec![1.0, 0.0, 0.0],
            metadata_json: json!({"category": "tech"}).to_string(),
            ..Default::default()
        }];
        upsert_test_points(&service, coll_name, points).await;

        // Explain SEM filtro
        let response = service
            .explain_search(Request::new(ExplainSearchRequest {
                collection: coll_name.to_string(),
                vector: vec![1.0, 0.0, 0.0],
                limit: 10,
                filter_json: String::new(),
                ..Default::default()
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.results.is_empty());

        // Sem filtro, filter_evaluation deve ser None
        for result in &resp.results {
            assert!(
                result.filter_evaluation.is_none(),
                "filter_evaluation deve ser None quando não há filtro"
            );
        }
    }
}
