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
use serde_json::json;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use ferres_db_core::{
    apply_delta, build_new_index, compact_wal_entries_older_than, estimate_index_size,
    needs_reindex, tombstone_ratio, Point, ReindexJob, ReindexStats, ReindexStatus,
};

use dashmap::DashMap;

use crate::api_err;
use crate::audit::{self, AuditResult};
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::time::unix_now;

/// Maximum number of completed/failed reindex jobs to keep in memory.
/// When this limit is exceeded, the oldest completed/failed jobs are evicted.
const MAX_COMPLETED_REINDEX_JOBS: usize = 50;

/// Threshold in bytes above which reindex logs a memory warning (~2 GB).
const REINDEX_MEMORY_WARNING_THRESHOLD: usize = 2_000_000_000;

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

/// Remove old completed/failed reindex jobs when the count exceeds `max_completed`.
///
/// Jobs with status `Queued`, `Building`, or `Swapping` are **never** removed.
/// Among finished jobs (Completed/Failed), the oldest ones (by `completed_at`)
/// are evicted first.
fn cleanup_old_reindex_jobs(jobs: &DashMap<String, Arc<RwLock<ReindexJob>>>, max_completed: usize) {
    // 1. Collect all finished jobs with their completed_at timestamps
    let mut finished: Vec<(String, u64)> = Vec::new();

    for entry in jobs.iter() {
        if let Ok(job) = entry.value().read() {
            if matches!(job.status, ReindexStatus::Completed | ReindexStatus::Failed) {
                let ts = job.completed_at.unwrap_or(0);
                finished.push((entry.key().clone(), ts));
            }
        }
    }

    // 2. If within limits, nothing to do
    if finished.len() <= max_completed {
        return;
    }

    // 3. Sort by completed_at ascending (oldest first)
    finished.sort_by_key(|&(_, ts)| ts);

    // 4. Remove the oldest entries that exceed the limit
    let to_remove = finished.len() - max_completed;
    for (job_id, _) in finished.into_iter().take(to_remove) {
        jobs.remove(&job_id);
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// Handler for POST /api/v1/collections/{name}/reindex
///
/// Starts a background reindex job for the specified collection.
/// Returns 409 if a reindex is already running for this collection.
///
/// **Memory note:** Reindex temporarily requires approximately 2× the
/// collection's memory: one copy for the existing index (serving queries)
/// and one for the new index being built from the snapshot.
/// For a 1M × 384 collection (~1.5 GB), expect ~3 GB peak usage.
pub async fn start_reindex(
    AuthenticatedUser(user): AuthenticatedUser,
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

    let estimated_memory = estimate_index_size(
        snapshot_points.len(),
        config.dimension,
        config.hnsw.max_nb_connection,
    ) * 2;
    if estimated_memory > REINDEX_MEMORY_WARNING_THRESHOLD {
        warn!(
            estimated_memory_gb = estimated_memory as f64 / 1e9,
            "reindex will require significant memory"
        );
    }

    // Populate initial stats
    job.stats.points_total = snapshot_points.len();
    job.stats.tombstones_cleaned = tombstone_count_before;
    job.stats.old_index_size_bytes = estimate_index_size(
        snapshot_points.len(),
        config.dimension,
        config.hnsw.max_nb_connection,
    );

    // Store job in AppState
    let job_arc = Arc::new(RwLock::new(job));
    app_state
        .reindex_jobs
        .insert(job_id.clone(), job_arc.clone());

    // Evict old completed/failed jobs to prevent unbounded memory growth
    cleanup_old_reindex_jobs(&app_state.reindex_jobs, MAX_COMPLETED_REINDEX_JOBS);

    // Audit: record reindex started (e.g. from Dashboard)
    {
        let entry = audit::audit_entry(
            &user.username,
            "reindex",
            &format!("collection:{name}"),
            json!({ "job_id": job_id, "collection": name, "points": snapshot_points.len(), "tombstones_cleaned": tombstone_count_before }),
            AuditResult::Success,
            None,
            None,
        );
        app_state.audit_logger.log(&entry);
    }

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

        let build_result =
            tokio::task::spawn_blocking(move || build_new_index(&config_clone, &snapshot_clone))
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
                    j.stats.new_index_size_bytes = estimate_index_size(
                        new_point_count,
                        config.dimension,
                        config.hnsw.max_nb_connection,
                    );
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
/// Called from the delete handler after removing points, or from the
/// background auto-reindex worker. This is a fire-and-forget operation —
/// errors are logged but not propagated.
pub fn maybe_auto_reindex(app_state: &AppState, collection_name: &str) {
    let collection_arc = match app_state.collections.get(collection_name) {
        Some(entry) => entry.value().clone(),
        None => return,
    };

    let (tombstones, total_indexed) = match collection_arc.read() {
        Ok(coll) => (coll.tombstone_count(), coll.total_indexed_len()),
        Err(_) => return,
    };

    if !needs_reindex(tombstones, total_indexed) {
        return;
    }

    // Don't trigger if already running
    if has_running_job(app_state, collection_name) {
        return;
    }

    let ratio = tombstone_ratio(tombstones, total_indexed);
    info!(
        collection = %collection_name,
        tombstones,
        total_indexed,
        ratio = format!("{:.1}%", ratio * 100.0),
        "auto-reindex triggered: tombstone threshold exceeded"
    );

    // Fire-and-forget: create and start the job
    let job_id = Uuid::new_v4().to_string();
    let mut job = ReindexJob::new(job_id.clone(), collection_name.to_string());

    let (snapshot_points, snapshot_ids, config, tombstone_count_before) =
        match collection_arc.read() {
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
    job.stats.old_index_size_bytes = estimate_index_size(
        snapshot_points.len(),
        config.dimension,
        config.hnsw.max_nb_connection,
    );

    let job_arc = Arc::new(RwLock::new(job));
    app_state
        .reindex_jobs
        .insert(job_id.clone(), job_arc.clone());

    // Evict old completed/failed jobs to prevent unbounded memory growth
    cleanup_old_reindex_jobs(&app_state.reindex_jobs, MAX_COMPLETED_REINDEX_JOBS);

    let job_id_bg = job_id;
    let name_bg = collection_name.to_string();
    let collection_arc_bg = collection_arc;

    tokio::task::spawn(async move {
        if let Ok(mut j) = job_arc.write() {
            j.set_building();
        }
        info!(
            job_id = %job_id_bg,
            collection = %name_bg,
            "auto-reindex compaction started"
        );

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
                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    status = "failed",
                    "auto-reindex compaction finished"
                );
                return;
            }
            Err(e) => {
                warn!(job_id = %job_id_bg, error = %e, "auto-reindex task panicked");
                if let Ok(mut j) = job_arc.write() {
                    j.set_failed(format!("task panicked: {e}"));
                }
                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    status = "failed",
                    "auto-reindex compaction finished"
                );
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
                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    "auto-reindex completed"
                );
                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    status = "completed",
                    "auto-reindex compaction finished"
                );
            }
            Err(e) => {
                warn!(job_id = %job_id_bg, error = %e, "auto-reindex swap failed");
                if let Ok(mut j) = job_arc.write() {
                    j.set_failed(e.clone());
                }
                info!(
                    job_id = %job_id_bg,
                    collection = %name_bg,
                    status = "failed",
                    "auto-reindex compaction finished"
                );
            }
        }
    });
}

/// Run one cycle of the auto-reindex worker: check all collections for
/// tombstone ratio and trigger reindex when above threshold (and no job running).
///
/// Called from the background task in main every 30 minutes.
pub fn run_auto_reindex_cycle(app_state: &AppState) {
    let num_collections = app_state.collections.len();
    info!(num_collections, "auto-reindex worker cycle started");

    for entry in app_state.collections.iter() {
        let name = entry.key().clone();
        let collection_arc = entry.value().clone();

        let (tombstone_count, total_indexed) = match collection_arc.read() {
            Ok(coll) => (coll.tombstone_count(), coll.total_indexed_len()),
            Err(_) => continue,
        };

        if total_indexed == 0 {
            continue;
        }

        if !needs_reindex(tombstone_count, total_indexed) {
            continue;
        }

        if has_running_job(app_state, &name) {
            debug!(
                collection = %name,
                "reindex already running, skipping"
            );
            continue;
        }

        info!(
            collection = %name,
            "auto-reindex triggered by background worker"
        );
        maybe_auto_reindex(app_state, &name);
    }

    info!("auto-reindex worker cycle finished");
}

/// Run one cycle of the TTL vacuum worker: remove expired points from all collections.
///
/// Called from the background task in main every 60 seconds.
pub fn run_auto_vacuum_cycle(app_state: &AppState) {
    let mut total_removed = 0usize;
    let mut skipped = 0usize;
    for entry in app_state.collections.iter() {
        let name = entry.key().clone();
        let collection_arc = entry.value().clone();
        let removed = match collection_arc.try_write() {
            Ok(mut coll) => coll.vacuum_expired_points(),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if removed > 0 {
            total_removed += removed;
            info!(
                collection = %name,
                removed,
                "TTL vacuum removed expired points"
            );
        }
    }
    if skipped > 0 {
        info!(
            skipped,
            "auto-vacuum skipped locked collections (will retry next cycle)"
        );
    }
    if total_removed > 0 {
        info!(total_removed, "auto-vacuum cycle finished");
    }
}

/// Run one cycle of the retention policy worker: compact WAL for collections
/// that have `retention_days` set, removing entries older than the configured period.
///
/// Called from the background task in main (e.g. every hour).
pub fn run_retention_cycle(app_state: &AppState) {
    let collections_dir = app_state.config.storage_path.join("collections");
    let now_secs = unix_now();
    let wal_compress = app_state.config.wal_compression;
    let mut total_removed = 0usize;

    for entry in app_state.collections.iter() {
        let name = entry.key();
        let collection_arc = entry.value();
        let retention_days = match collection_arc.read() {
            Ok(coll) => coll.config().retention_days,
            Err(_) => continue,
        };
        let Some(days) = retention_days else { continue };
        let cutoff_ts = now_secs.saturating_sub(days as u64 * 86400);
        let collection_dir = collections_dir.join(name);

        match app_state
            .storage_circuit_breaker
            .call(|| compact_wal_entries_older_than(&collection_dir, cutoff_ts, wal_compress))
        {
            Ok(removed) => {
                if removed > 0 {
                    total_removed += removed;
                    info!(
                        collection = %name,
                        removed,
                        retention_days = days,
                        "retention policy compacted WAL"
                    );
                }
            }
            Err(e) => {
                warn!(
                    collection = %name,
                    error = %e,
                    "retention policy WAL compaction failed"
                );
            }
        }
    }

    if total_removed > 0 {
        info!(total_removed, "retention cycle finished");
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a completed job with a specific `completed_at` timestamp.
    fn make_completed_job(id: &str, completed_at: u64) -> Arc<RwLock<ReindexJob>> {
        let mut job = ReindexJob::new(id.to_string(), "test_col".to_string());
        job.status = ReindexStatus::Completed;
        job.progress = 1.0;
        job.completed_at = Some(completed_at);
        Arc::new(RwLock::new(job))
    }

    /// Helper: create a failed job with a specific `completed_at` timestamp.
    fn make_failed_job(id: &str, completed_at: u64) -> Arc<RwLock<ReindexJob>> {
        let mut job = ReindexJob::new(id.to_string(), "test_col".to_string());
        job.status = ReindexStatus::Failed;
        job.error = Some("test error".to_string());
        job.completed_at = Some(completed_at);
        Arc::new(RwLock::new(job))
    }

    #[test]
    fn test_cleanup_removes_old_completed_jobs() {
        let jobs: DashMap<String, Arc<RwLock<ReindexJob>>> = DashMap::new();

        // Insert 60 completed jobs with sequential timestamps
        for i in 0..60 {
            let id = format!("job-{i}");
            jobs.insert(id.clone(), make_completed_job(&id, 1000 + i as u64));
        }

        assert_eq!(jobs.len(), 60);

        cleanup_old_reindex_jobs(&jobs, 50);

        // Should keep exactly 50
        assert_eq!(jobs.len(), 50);

        // The 10 oldest (job-0 .. job-9) should have been removed
        for i in 0..10 {
            assert!(
                !jobs.contains_key(&format!("job-{i}")),
                "job-{i} should have been removed (oldest)"
            );
        }

        // The 50 newest (job-10 .. job-59) should still be present
        for i in 10..60 {
            assert!(
                jobs.contains_key(&format!("job-{i}")),
                "job-{i} should still be present"
            );
        }
    }

    #[test]
    fn test_cleanup_preserves_running_jobs() {
        let jobs: DashMap<String, Arc<RwLock<ReindexJob>>> = DashMap::new();

        // Insert 55 completed jobs
        for i in 0..55 {
            let id = format!("completed-{i}");
            jobs.insert(id.clone(), make_completed_job(&id, 1000 + i as u64));
        }

        // Insert active jobs (Queued, Building, Swapping) — these must NEVER be removed
        let mut queued_job = ReindexJob::new("queued-1".to_string(), "test_col".to_string());
        queued_job.status = ReindexStatus::Queued;
        jobs.insert("queued-1".to_string(), Arc::new(RwLock::new(queued_job)));

        let mut building_job = ReindexJob::new("building-1".to_string(), "test_col".to_string());
        building_job.status = ReindexStatus::Building;
        jobs.insert(
            "building-1".to_string(),
            Arc::new(RwLock::new(building_job)),
        );

        let mut swapping_job = ReindexJob::new("swapping-1".to_string(), "test_col".to_string());
        swapping_job.status = ReindexStatus::Swapping;
        jobs.insert(
            "swapping-1".to_string(),
            Arc::new(RwLock::new(swapping_job)),
        );

        // Also add a few failed jobs
        for i in 0..5 {
            let id = format!("failed-{i}");
            jobs.insert(id.clone(), make_failed_job(&id, 500 + i as u64));
        }

        // Total: 55 completed + 3 active + 5 failed = 63
        assert_eq!(jobs.len(), 63);

        // Finished = 55 completed + 5 failed = 60, limit = 50 → remove 10 oldest finished
        cleanup_old_reindex_jobs(&jobs, 50);

        // Active jobs must still be present
        assert!(
            jobs.contains_key("queued-1"),
            "queued job must not be removed"
        );
        assert!(
            jobs.contains_key("building-1"),
            "building job must not be removed"
        );
        assert!(
            jobs.contains_key("swapping-1"),
            "swapping job must not be removed"
        );

        // Count remaining finished jobs
        let finished_count: usize = jobs
            .iter()
            .filter(|entry| {
                if let Ok(job) = entry.value().read() {
                    matches!(job.status, ReindexStatus::Completed | ReindexStatus::Failed)
                } else {
                    false
                }
            })
            .count();

        assert_eq!(finished_count, 50, "should keep exactly 50 finished jobs");

        // Total = 50 finished + 3 active = 53
        assert_eq!(jobs.len(), 53);
    }

    #[test]
    fn test_cleanup_noop_under_limit() {
        let jobs: DashMap<String, Arc<RwLock<ReindexJob>>> = DashMap::new();

        // Insert only 30 completed jobs (under limit of 50)
        for i in 0..30 {
            let id = format!("job-{i}");
            jobs.insert(id.clone(), make_completed_job(&id, 1000 + i as u64));
        }

        assert_eq!(jobs.len(), 30);

        cleanup_old_reindex_jobs(&jobs, 50);

        // Nothing should be removed — all 30 should remain
        assert_eq!(jobs.len(), 30);

        for i in 0..30 {
            assert!(
                jobs.contains_key(&format!("job-{i}")),
                "job-{i} should still be present (under limit)"
            );
        }
    }
}
