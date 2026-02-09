//! # MCP (Model Context Protocol) server
//!
//! Embedded MCP server via STDIO. Exposes FerresDB operations as MCP tools:
//! `search_points`, `upsert_points`, and `get_stats`.

use std::sync::Arc;

use ferres_db_core::{MetadataFilter, Point};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer, ServiceExt};
use serde_json::json;
use tracing::warn;

use crate::request_validation::{
    validate_search_limit, validate_vector_dimension, MAX_POINTS_PER_BATCH,
    MAX_VECTOR_DIM,
};
use crate::state::AppState;

const TOOL_SEARCH_POINTS: &str = "search_points";
const TOOL_UPSERT_POINTS: &str = "upsert_points";
const TOOL_GET_STATS: &str = "get_stats";

/// MCP server handler that dispatches tool calls to FerresDB state.
#[derive(Clone)]
pub struct FerresMcpHandler {
    pub app_state: Arc<AppState>,
}

fn schema_object(value: serde_json::Value) -> Arc<serde_json::Map<String, serde_json::Value>> {
    Arc::new(value.as_object().cloned().unwrap_or_default())
}

impl FerresMcpHandler {
    pub fn new(app_state: Arc<AppState>) -> Self {
        Self { app_state }
    }

    fn tool_definitions() -> Vec<Tool> {
        vec![
            Tool::new(
                TOOL_SEARCH_POINTS,
                "Vector similarity search in a collection. Uses native HNSW pre-filtering when filter or namespace is provided.",
                schema_object(json!({
                    "type": "object",
                    "properties": {
                        "collection": { "type": "string", "description": "Collection name" },
                        "vector": { "type": "array", "items": { "type": "number" }, "description": "Query vector" },
                        "limit": { "type": "integer", "description": "Max results (1..10000)" },
                        "filter": { "type": "object", "description": "Optional metadata filter (JSON)" },
                        "namespace": { "type": "string", "description": "Optional namespace" },
                        "vector_field": { "type": "string", "description": "Optional vector field name" }
                    },
                    "required": ["collection", "vector", "limit"]
                })),
            ),
            Tool::new(
                TOOL_UPSERT_POINTS,
                "Insert or update points in a collection. No auth when using MCP (trusted channel).",
                schema_object(json!({
                    "type": "object",
                    "properties": {
                        "collection": { "type": "string", "description": "Collection name" },
                        "points": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "vector": { "type": "array", "items": { "type": "number" } },
                                    "metadata": { "type": "object" },
                                    "namespace": { "type": "string" },
                                    "ttl": { "type": "integer" }
                                },
                                "required": ["id", "vector"]
                            }
                        }
                    },
                    "required": ["collection", "points"]
                })),
            ),
            Tool::new(
                TOOL_GET_STATS,
                "Get statistics: global (omit collection) or per-collection (pass collection name).",
                schema_object(json!({
                    "type": "object",
                    "properties": {
                        "collection": { "type": "string", "description": "Optional collection name for per-collection stats" }
                    }
                })),
            ),
        ]
    }
}

impl ServerHandler for FerresMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "FerresDB".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(
            Self::tool_definitions(),
        )))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + Send + '_ {
        let app_state = self.app_state.clone();
        let name = request.name.to_string();
        let arguments = request.arguments.clone();
        async move {
            let empty = serde_json::Map::new();
            let args = arguments.as_ref().unwrap_or(&empty);
            let result = match name.as_str() {
                TOOL_SEARCH_POINTS => do_search_points(&app_state, args).await,
                TOOL_UPSERT_POINTS => do_upsert_points(&app_state, args).await,
                TOOL_GET_STATS => do_get_stats(&app_state, args).await,
                _ => Err(format!("unknown tool: {}", name)),
            };
            match result {
                Ok(json_val) => Ok(CallToolResult::structured(json_val)),
                Err(msg) => Ok(CallToolResult::structured_error(
                    json!({ "error": "tool_error", "message": msg }),
                )),
            }
        }
    }
}

async fn do_search_points(
    app_state: &AppState,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let collection_name = args
        .get("collection")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required argument: collection".to_string())?;
    let vector: Vec<f32> = args
        .get("vector")
        .and_then(|v| {
            v.as_array()
                .map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
        })
        .ok_or_else(|| "missing or invalid argument: vector (array of numbers)".to_string())?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .ok_or_else(|| "missing or invalid argument: limit".to_string())?;

    validate_search_limit(limit).map_err(|e| e.message().to_string())?;
    validate_vector_dimension(&vector).map_err(|e| e.message().to_string())?;

    let collection_arc = app_state
        .collections
        .get(collection_name)
        .ok_or_else(|| format!("collection not found: {}", collection_name))?;

    let collection = collection_arc.read().map_err(|e| e.to_string())?;
    collection
        .validate_dimension(&vector)
        .map_err(|e| e.to_string())?;

    let filter_json = args.get("filter").cloned();
    let namespace = args.get("namespace").and_then(|v| v.as_str()).map(String::from);
    let vector_field = args
        .get("vector_field")
        .and_then(|v| v.as_str())
        .map(String::from);

    let mut filter = match filter_json {
        Some(fv) => MetadataFilter::from_json(fv).map_err(|e| e.to_string())?,
        None => MetadataFilter::empty(),
    };
    if let Some(ref ns) = namespace {
        filter.namespace = Some(ns.clone());
    }
    let vector_field_ref = vector_field.as_deref();
    let results = if filter.is_empty() {
        collection
            .search(&vector, limit, None, vector_field_ref)
            .map_err(|e| e.to_string())?
    } else {
        let predicate = |id: &str| {
            collection
                .get(id)
                .map(|p| filter.matches_point(&p))
                .unwrap_or(false)
        };
        collection
            .search(&vector, limit, Some(&predicate), vector_field_ref)
            .map_err(|e| e.to_string())?
    };

    let search_results: Vec<serde_json::Value> = results
        .into_iter()
        .filter_map(|(storage_id, score)| {
            let point = collection.get(&storage_id)?;
            Some(json!({
                "id": point.id,
                "score": score,
                "metadata": point.metadata,
                "namespace": point.namespace
            }))
        })
        .collect();

    Ok(json!({ "results": search_results }))
}

async fn do_upsert_points(
    app_state: &AppState,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let collection_name = args
        .get("collection")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required argument: collection".to_string())?;
    let points_arr = args
        .get("points")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing or invalid argument: points (array)".to_string())?;

    if points_arr.len() > MAX_POINTS_PER_BATCH {
        return Err(format!(
            "batch too large: max {} points per request",
            MAX_POINTS_PER_BATCH
        ));
    }

    let collection_arc = app_state
        .collections
        .get(collection_name)
        .ok_or_else(|| format!("collection not found: {}", collection_name))?;

    let mut collection = collection_arc.write().map_err(|e| e.to_string())?;
    let mut points = Vec::new();
    let mut failed = Vec::new();

    for (i, pv) in points_arr.iter().enumerate() {
        let obj = pv.as_object().ok_or_else(|| {
            format!("points[{}]: expected object", i)
        })?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("points[{}]: missing id", i))?
            .to_string();
        let vector: Vec<f32> = obj
            .get("vector")
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_f64().map(|f| f as f32))
                        .collect()
                })
            })
            .ok_or_else(|| format!("points[{}]: missing or invalid vector", i))?;

        if vector.len() > MAX_VECTOR_DIM {
            failed.push(json!({ "id": id, "reason": format!("vector dimension too large: max {}", MAX_VECTOR_DIM) }));
            continue;
        }
        if vector.is_empty() {
            failed.push(json!({ "id": id, "reason": "vector cannot be empty" }));
            continue;
        }

        if let Err(e) = collection.validate_dimension(&vector) {
            failed.push(json!({ "id": id, "reason": e.to_string() }));
            continue;
        }

        let metadata = obj.get("metadata").cloned().unwrap_or(json!(null));
        let point = match Point::new(id.clone(), vector, metadata) {
            Ok(mut point) => {
                point.namespace = obj.get("namespace").and_then(|v| v.as_str()).map(String::from);
                if let Some(ttl) = obj.get("ttl").and_then(|v| v.as_u64()) {
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    point.expires_at = Some(now_secs.saturating_add(ttl));
                }
                point
            }
            Err(e) => {
                failed.push(json!({ "id": id, "reason": e.to_string() }));
                continue;
            }
        };
        points.push(point);
    }

    let upserted = if points.is_empty() {
        0
    } else {
        match collection.insert_batch(points) {
            Ok(result) => {
                collection.mark_dirty();
                result.inserted
            }
            Err(err) => {
                failed.push(json!({ "id": "batch", "reason": err.to_string() }));
                0
            }
        }
    };

    if upserted > 0 {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        app_state.record_ingest(now_secs, upserted as u64);
    }

    Ok(json!({
        "upserted": upserted,
        "failed": failed
    }))
}

async fn do_get_stats(
    app_state: &AppState,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let collection_name = args.get("collection").and_then(|v| v.as_str());

    if let Some(name) = collection_name {
        let collection_arc = app_state
            .collections
            .get(name)
            .ok_or_else(|| format!("collection not found: {}", name))?;
        let (num_points, tombstone_count, tombstone_memory_waste_bytes) = {
            let collection = collection_arc.read().map_err(|e| e.to_string())?;
            (
                collection.len(),
                collection.tombstone_count(),
                collection.tombstone_memory_waste(),
            )
        };
        let (num_queries, avg_latency_ms, p50_latency_ms, p95_latency_ms, p99_latency_ms) = app_state
            .query_stats
            .get(name)
            .map(|s| {
                let num_queries = s.num_queries.load(std::sync::atomic::Ordering::Relaxed);
                let (avg, p50, p95, p99) = s.calculate_percentiles();
                (num_queries, avg, p50, p95, p99)
            })
            .unwrap_or((0, 0.0, 0.0, 0.0, 0.0));

        return Ok(json!({
            "num_points": num_points,
            "num_queries": num_queries,
            "avg_latency_ms": avg_latency_ms,
            "p50_latency_ms": p50_latency_ms,
            "p95_latency_ms": p95_latency_ms,
            "p99_latency_ms": p99_latency_ms,
            "tombstone_count": tombstone_count,
            "tombstone_memory_waste_bytes": tombstone_memory_waste_bytes
        }));
    }

    let total_collections = app_state.collections.len();
    let total_points: usize = app_state
        .collections
        .iter()
        .map(|entry| entry.value().read().map(|c| c.len()).unwrap_or(0))
        .sum();
    let cache = &app_state.query_log_cache;
    let total_queries_24h = cache.total_queries_24h();
    let avg_latency_ms = cache.avg_latency_24h();
    let queries_per_minute: Vec<serde_json::Value> = cache
        .queries_per_minute_24h()
        .into_iter()
        .map(|(timestamp, count)| json!({ "timestamp": timestamp, "count": count }))
        .collect();

    Ok(json!({
        "total_collections": total_collections,
        "total_points": total_points,
        "total_queries_24h": total_queries_24h,
        "avg_latency_ms": avg_latency_ms,
        "queries_per_minute": queries_per_minute,
        "simd_enabled": ferres_db_core::simd_enabled()
    }))
}

/// Spawns the MCP server in a dedicated Tokio task. Uses stdin/stdout for the protocol.
/// Must be called only when MCP mode is enabled; logs should be directed to stderr
/// so that stdout is reserved for MCP messages.
pub fn spawn_mcp_server(app_state: Arc<AppState>) {
    let handler = FerresMcpHandler::new(app_state);
    tokio::spawn(async move {
        let transport = (tokio::io::stdin(), tokio::io::stdout());
        match ServiceExt::serve(handler, transport).await {
            Ok(running) => {
                if let Err(e) = running.waiting().await {
                    warn!(error = %e, "MCP server task ended with error");
                }
            }
            Err(e) => {
                warn!(error = %e, "MCP server failed to start");
            }
        }
    });
}
