//! # Reindex Handlers — background index rebuild endpoints
//!
//! Provides handlers to start, monitor and list background reindex jobs
//! for a collection. At most one reindex job may run per collection.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Serialize;
use tracing::{info, warn, error};
use uuid::Uuid;

use ferres_db_core::{
    ReindexJob, ReindexStatus, ReindexStats,
    apply_delta, build_new_index, estimate_index_size, needs_reindex,
    Point,
};

use crate::api_err;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Response types ──────────────────────────────────────────────────────

/// Response for POST /api/v1/collections/{name}/reindex
#[derive(Debug, Serialize)]
pub struct StartReindexResponse {
    pub job_id: String,
    pub collection: String,
    pub status: ReindexStatus,
    pub message: String,
}

/// Response for GET /api/v1/collections/{name}/reindex/{job_id}
#[derive(Debug, Serialize)]
pub struct ReindexJobResponse {
    pub id: String,
    pub collection: String,
    pub status: ReindexStatus,
    pub progress: f64,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub error: Option<String>,
    pub stats: ReindexStats,
}

/// Response for GET /api/v1/collections/{name}/reindex
#[derive(Debug, Serialize)]
pub struct ListReindexJobsResponse {
    pub jobs: Vec<ReindexJobResponse>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn job_to_response(job: &ReindexJob) -> ReindexJobResponse {
    ReindexJobResponse {
        id: job.id.clone(),
        collection: job.collection.clone(),
        status: job.status.clone(),
        progress: job.progress,
        started_at: job.started_at,
        completed_at: job.completed_at,
        error: job.error.clone(),
        stats: job.stats.clone(),
    }
}

/// Check if a reindex job is already running for the given collection.
fn has_running_job(app_state: &AppState, collection: &str) -> bool {
    for entry in app_state.reindex_jobs.iter() {
        if let Ok(job) = entry.value().read() {
            if job.collection == collection
                && matches!(
                    job.status,
                    ReindexStatus::Queued | ReindexStatus::Building | ReindexStatus::Swapping
                )
            {
                return true;
            }
        }
    }
    false
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// Handler for POST /api/v1/collections/{name}/reindex
///
/// Starts a background reindex job for the specified collection.
/// Returns 409 if a reindex is already running for this collection.
pub async fn start_reindex(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<(StatusCode, Json<StartReindexResponse>)> {
    // Validate collection exists
    let collection_arc = app_state
        .collections
        .get(&name)
        .ok_or_else(|| ApiError::collection_not_found(&name))?
        .value()
        .clone();

    // Check for existing running job
    if has_running_job(&app_state, &name) {
        return Err(ApiError::CollectionAlreadyExists {
            message: format!("a reindex job is already running for collection '{name}'"),
        });
    }

    // Create the job
    let job_id = Uuid::new_v4().to_string();
    let mut job = ReindexJob::new(job_id.clone(), name.clone());

    // Snapshot points while holding a short read lock
    let (snapshot_points, snapshot_ids, config, tombstone_count_before) = {
        let coll = api_err!(collection_arc.read(), "failed to acquire read lock")?;
        let (pts, ids) = coll.points_snapshot();
        let cfg = coll.config().clone();
        let tc = coll.tombstone_count();
        (pts, ids, cfg, tc)
    };

    // Populate initial stats
    job.stats.points_total = snapshot_points.len();
    job.stats.tombstones_cleaned = tombstone_count_before;
    job.stats.old_index_size_bytes =
        estimate_index_size(snapshot_points.len(), config.dimension, config.hnsw.max_nb_connection);

    // Store job in AppState
    let job_arc = Arc::new(RwLock::new(job));
    app_state
        .reindex_jobs
        .insert(job_id.clone(), job_arc.clone());

    info!(
        job_id = %job_id,
        collection = %name,
        points = snapshot_points.len(),
        tombstones = tombstone_count_before,
        "reindex job created, starting background build"
    );

    // Spawn background task
    let job_arc_bg = job_arc.clone();
    let collection_arc_bg = collection_arc.clone();
    let name_bg = name.clone();
    let job_id_bg = job_id.clone();

    tokio::task::spawn(async move {
        // Phase 1: Building (CPU-intensive, use spawn_blocking)
        {
            if let Ok(mut j) = job_arc_bg.write() {
                j.set_building();
            }
        }

        let config_clone = config.clone();
        let snapshot_clone = snapshot_points.clone();

        let build_result = tokio::task::spawn_blocking(move || {
            build_new_index(&config_clone, &snapshot_clone)
        })
        .await;

        let mut new_index = match build_result {
            Ok(Ok(idx)) => idx,
            Ok(Err(e)) => {
                error!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    error = %e,
                    "reindex build failed"
                );
                if let Ok(mut j) = job_arc_bg.write() {
                    j.set_failed(format!("index build failed: {e}"));
                }
                return;
            }
            Err(e) => {
                error!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    error = %e,
                    "reindex task panicked"
                );
                if let Ok(mut j) = job_arc_bg.write() {
                    j.set_failed(format!("task panicked: {e}"));
                }
                return;
            }
        };

        // Update progress
        if let Ok(mut j) = job_arc_bg.write() {
            j.stats.points_processed = j.stats.points_total;
            j.set_progress(0.90);
        }

        // Phase 2: Swapping (brief write lock)
        {
            if let Ok(mut j) = job_arc_bg.write() {
                j.set_swapping();
            }
        }

        // Apply delta and swap under write lock
        let swap_result: Result<(), String> = (|| {
            let mut coll = collection_arc_bg
                .write()
                .map_err(|e| format!("failed to acquire write lock: {e}"))?;

            // Build current_points map for delta
            let current_points: HashMap<String, Point> = coll
                .points_owned()
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect();

            // Apply delta (additions/removals during build)
            let (added, removed) = apply_delta(&mut new_index, &snapshot_ids, &current_points)
                .map_err(|e| format!("delta application failed: {e}"))?;

            info!(
                job_id = %job_id_bg,
                collection = %name_bg,
                added,
                removed,
                "delta applied, swapping index"
            );

            // Swap the index (< 1ms critical section)
            coll.swap_index(new_index);

            Ok(())
        })();

        match swap_result {
            Ok(()) => {
                // Phase 3: Cleanup (old index dropped automatically)
                if let Ok(mut j) = job_arc_bg.write() {
                    let coll = collection_arc_bg.read().ok();
                    let new_point_count = coll.as_ref().map(|c| c.len()).unwrap_or(0);
                    j.stats.new_index_size_bytes =
                        estimate_index_size(new_point_count, config.dimension, config.hnsw.max_nb_connection);
                    j.set_completed();
                }

                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    "reindex completed successfully"
                );
            }
            Err(e) => {
                error!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    error = %e,
                    "reindex swap failed"
                );
                if let Ok(mut j) = job_arc_bg.write() {
                    j.set_failed(e);
                }
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(StartReindexResponse {
            job_id,
            collection: name,
            status: ReindexStatus::Building,
            message: "reindex job started".to_string(),
        }),
    ))
}

/// Handler for GET /api/v1/collections/{name}/reindex/{job_id}
///
/// Returns the status of a specific reindex job.
pub async fn get_reindex_job(
    State(app_state): State<AppState>,
    Path((name, job_id)): Path<(String, String)>,
) -> ApiResult<Json<ReindexJobResponse>> {
    // Validate collection exists
    if !app_state.collections.contains_key(&name) {
        return Err(ApiError::collection_not_found(&name));
    }

    let job_arc = app_state
        .reindex_jobs
        .get(&job_id)
        .ok_or_else(|| ApiError::CollectionNotFound {
            message: format!("reindex job '{job_id}' not found"),
        })?
        .value()
        .clone();

    let job = api_err!(job_arc.read(), "failed to read reindex job")?;

    // Verify job belongs to the requested collection
    if job.collection != name {
        return Err(ApiError::CollectionNotFound {
            message: format!("reindex job '{job_id}' not found for collection '{name}'"),
        });
    }

    Ok(Json(job_to_response(&job)))
}

/// Handler for GET /api/v1/collections/{name}/reindex
///
/// Lists all reindex jobs for a collection.
pub async fn list_reindex_jobs(
    State(app_state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ListReindexJobsResponse>> {
    // Validate collection exists
    if !app_state.collections.contains_key(&name) {
        return Err(ApiError::collection_not_found(&name));
    }

    let mut jobs = Vec::new();
    for entry in app_state.reindex_jobs.iter() {
        if let Ok(job) = entry.value().read() {
            if job.collection == name {
                jobs.push(job_to_response(&job));
            }
        }
    }

    // Sort by started_at descending (newest first)
    jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(Json(ListReindexJobsResponse { jobs }))
}

/// Trigger an automatic reindex if tombstones exceed the threshold.
///
/// Called from the delete handler after removing points. This is a
/// fire-and-forget operation — errors are logged but not propagated.
pub fn maybe_auto_reindex(app_state: &AppState, collection_name: &str) {
    let collection_arc = match app_state.collections.get(collection_name) {
        Some(entry) => entry.value().clone(),
        None => return,
    };

    let (tombstones, total) = match collection_arc.read() {
        Ok(coll) => (coll.tombstone_count(), coll.len()),
        Err(_) => return,
    };

    if !needs_reindex(tombstones, total) {
        return;
    }

    // Don't trigger if already running
    if has_running_job(app_state, collection_name) {
        return;
    }

    info!(
        collection = %collection_name,
        tombstones,
        total,
        ratio = format!("{:.1}%", (tombstones as f64 / total.max(1) as f64) * 100.0),
        "auto-reindex triggered: tombstone threshold exceeded"
    );

    // Fire-and-forget: create and start the job
    let job_id = Uuid::new_v4().to_string();
    let mut job = ReindexJob::new(job_id.clone(), collection_name.to_string());

    let (snapshot_points, snapshot_ids, config, tombstone_count_before) = match collection_arc.read()
    {
        Ok(coll) => {
            let (pts, ids) = coll.points_snapshot();
            let cfg = coll.config().clone();
            let tc = coll.tombstone_count();
            (pts, ids, cfg, tc)
        }
        Err(_) => return,
    };

    job.stats.points_total = snapshot_points.len();
    job.stats.tombstones_cleaned = tombstone_count_before;
    job.stats.old_index_size_bytes =
        estimate_index_size(snapshot_points.len(), config.dimension, config.hnsw.max_nb_connection);

    let job_arc = Arc::new(RwLock::new(job));
    app_state
        .reindex_jobs
        .insert(job_id.clone(), job_arc.clone());

    let job_id_bg = job_id;
    let name_bg = collection_name.to_string();
    let collection_arc_bg = collection_arc;

    tokio::task::spawn(async move {
        if let Ok(mut j) = job_arc.write() {
            j.set_building();
        }

        let config_clone = config.clone();
        let snapshot_clone = snapshot_points;

        let build_result =
            tokio::task::spawn_blocking(move || build_new_index(&config_clone, &snapshot_clone))
                .await;

        let mut new_index = match build_result {
            Ok(Ok(idx)) => idx,
            Ok(Err(e)) => {
                warn!(job_id = %job_id_bg, error = %e, "auto-reindex build failed");
                if let Ok(mut j) = job_arc.write() {
                    j.set_failed(format!("build failed: {e}"));
                }
                return;
            }
            Err(e) => {
                warn!(job_id = %job_id_bg, error = %e, "auto-reindex task panicked");
                if let Ok(mut j) = job_arc.write() {
                    j.set_failed(format!("task panicked: {e}"));
                }
                return;
            }
        };

        if let Ok(mut j) = job_arc.write() {
            j.stats.points_processed = j.stats.points_total;
            j.set_swapping();
        }

        let swap_result: Result<(), String> = (|| {
            let mut coll = collection_arc_bg
                .write()
                .map_err(|e| format!("write lock: {e}"))?;

            let current_points: HashMap<String, Point> = coll
                .points_owned()
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect();

            apply_delta(&mut new_index, &snapshot_ids, &current_points)
                .map_err(|e| format!("delta: {e}"))?;

            coll.swap_index(new_index);
            Ok(())
        })();

        match swap_result {
            Ok(()) => {
                if let Ok(mut j) = job_arc.write() {
                    let coll = collection_arc_bg.read().ok();
                    let count = coll.as_ref().map(|c| c.len()).unwrap_or(0);
                    j.stats.new_index_size_bytes =
                        estimate_index_size(count, config.dimension, config.hnsw.max_nb_connection);
                    j.set_completed();
                }
                info!(job_id = %job_id_bg, collection = %name_bg, "auto-reindex completed");
            }
            Err(e) => {
                warn!(job_id = %job_id_bg, error = %e, "auto-reindex swap failed");
                if let Ok(mut j) = job_arc.write() {
                    j.set_failed(e);
                }
            }
        }
    });
}
