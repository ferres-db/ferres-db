//! # Reindex — background index rebuild without downtime
//!
//! Provides types and logic for rebuilding a collection's ANN index
//! in the background without blocking searches or mutations.
//!
//! ## Flow
//!
//! 1. **Building**: Snapshot current points, build new index in a separate thread.
//!    Searches continue using the old index. Writes go to the collection normally.
//! 2. **Swapping**: Brief write lock (< 1 ms) to swap old index for new one.
//!    Delta operations (inserts/deletes during building) are applied to the new index.
//! 3. **Cleanup**: Old index is dropped, freeing memory.
//!
//! ## Memory
//!
//! Reindex temporarily requires approximately 2× the collection's memory: one
//! copy for the existing index (serving queries) and one for the new index
//! being built from the snapshot. For a 1M × 384 collection (~1.5 GB), expect
//! ~3 GB peak usage.
//!
//! ## Auto-reindex
//!
//! When tombstones exceed 20 % of indexed points, a reindex is recommended.
//! The server can trigger this automatically after delete operations or via a
//! background worker. For [`needs_reindex`], the caller must pass
//! `total_indexed` = number of entries in the index = live points + tombstones,
//! i.e. `collection.len() + collection.tombstone_count()` (or
//! `collection.total_indexed_len()` if available).

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::collection::CollectionConfig;
use crate::error::FerresError;
use crate::point::Point;
use crate::search::{create_ann_index, ANNIndex};

// ─── Constants ──────────────────────────────────────────────────────

/// Auto-reindex threshold: when tombstones exceed this ratio of total
/// indexed points, an automatic reindex is recommended.
pub const AUTO_REINDEX_TOMBSTONE_RATIO: f64 = 0.20;

// ─── ReindexStatus ──────────────────────────────────────────────────

/// Status of a reindex job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReindexStatus {
    /// Job is queued, waiting to start.
    Queued,
    /// Building new index in background.
    Building,
    /// Swapping old index for new one (brief lock).
    Swapping,
    /// Reindex completed successfully.
    Completed,
    /// Reindex failed with an error.
    Failed,
}

// ─── ReindexStats ───────────────────────────────────────────────────

/// Statistics about a reindex operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReindexStats {
    /// Number of points processed so far.
    pub points_processed: usize,
    /// Total number of points to process.
    pub points_total: usize,
    /// Number of tombstones cleaned by the reindex.
    pub tombstones_cleaned: usize,
    /// Estimated size of the old index in bytes.
    pub old_index_size_bytes: usize,
    /// Estimated size of the new index in bytes.
    pub new_index_size_bytes: usize,
}

// ─── ReindexJob ─────────────────────────────────────────────────────

/// A reindex job tracking the progress of a background index rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReindexJob {
    /// Unique identifier for this job.
    pub id: String,
    /// Name of the collection being reindexed.
    pub collection: String,
    /// Current status of the job.
    pub status: ReindexStatus,
    /// Progress from 0.0 to 1.0.
    pub progress: f64,
    /// Unix timestamp (seconds) when the job was started.
    pub started_at: u64,
    /// Unix timestamp (seconds) when the job completed (or failed).
    pub completed_at: Option<u64>,
    /// Error message if the job failed.
    pub error: Option<String>,
    /// Statistics about the reindex operation.
    pub stats: ReindexStats,
}

impl ReindexJob {
    /// Create a new reindex job in Queued status.
    pub fn new(id: String, collection: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Self {
            id,
            collection,
            status: ReindexStatus::Queued,
            progress: 0.0,
            started_at: now,
            completed_at: None,
            error: None,
            stats: ReindexStats::default(),
        }
    }

    /// Transition to Building status.
    pub fn set_building(&mut self) {
        self.status = ReindexStatus::Building;
    }

    /// Update progress (clamped to 0.0 .. 1.0).
    pub fn set_progress(&mut self, progress: f64) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    /// Transition to Swapping status.
    pub fn set_swapping(&mut self) {
        self.status = ReindexStatus::Swapping;
        self.progress = 0.95;
    }

    /// Mark the job as completed successfully.
    pub fn set_completed(&mut self) {
        self.status = ReindexStatus::Completed;
        self.progress = 1.0;
        self.completed_at = Some(now_secs());
    }

    /// Mark the job as failed with an error message.
    pub fn set_failed(&mut self, error: String) {
        self.status = ReindexStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(now_secs());
    }
}

// ─── Core reindex logic ─────────────────────────────────────────────

/// Build a new ANN index from the given points and configuration.
///
/// This is the core of the background reindex: it creates a fresh index
/// without tombstones. Called in a separate thread to avoid blocking.
pub fn build_new_index(
    config: &CollectionConfig,
    points: &[Point],
) -> Result<Box<dyn ANNIndex>, FerresError> {
    let mut new_index =
        create_ann_index(config.distance, config.hnsw.clone(), &config.quantization);
    new_index.build(points)?;
    Ok(new_index)
}

/// Apply delta operations to a newly built index.
///
/// Compares the snapshot point IDs with the current point IDs to determine
/// which points were added or removed during the building phase, then
/// applies the delta to the new index.
///
/// Returns `(added, removed)` counts.
pub fn apply_delta(
    new_index: &mut Box<dyn ANNIndex>,
    snapshot_ids: &HashSet<String>,
    current_points: &HashMap<String, Point>,
) -> Result<(usize, usize), FerresError> {
    let current_ids: HashSet<&String> = current_points.keys().collect();

    // Points added during building (in current but not in snapshot)
    let mut added = 0usize;
    for (id, point) in current_points {
        if !snapshot_ids.contains(id) {
            new_index.add_point(point)?;
            added += 1;
        }
    }

    // Points removed during building (in snapshot but not in current)
    let mut removed = 0usize;
    for id in snapshot_ids {
        if !current_ids.contains(id) {
            new_index.remove_point(id);
            removed += 1;
        }
    }

    debug!(added, removed, "delta applied to new index");
    Ok((added, removed))
}

/// Check if a collection needs reindexing based on tombstone ratio.
///
/// Returns `true` if tombstones > 20 % of total indexed points.
///
/// **`total_indexed`** must be the number of entries in the index (live + tombstones),
/// e.g. `collection.len() + collection.tombstone_count()` or `collection.total_indexed_len()`.
pub fn needs_reindex(tombstone_count: usize, total_indexed: usize) -> bool {
    if total_indexed == 0 {
        return false;
    }
    let ratio = tombstone_count as f64 / total_indexed as f64;
    ratio > AUTO_REINDEX_TOMBSTONE_RATIO
}

/// Tombstone ratio for logging and metrics: `tombstone_count / total_indexed`.
///
/// Returns 0.0 when `total_indexed` is 0. Callers should pass
/// `total_indexed = len() + tombstone_count()` (or `total_indexed_len()`).
pub fn tombstone_ratio(tombstone_count: usize, total_indexed: usize) -> f64 {
    if total_indexed == 0 {
        return 0.0;
    }
    tombstone_count as f64 / total_indexed as f64
}

/// Estimate the size of an index in bytes.
///
/// Approximation: vector storage + HNSW overhead.
pub fn estimate_index_size(num_points: usize, dimension: usize, m: usize) -> usize {
    let vector_size = num_points * dimension * 4; // f32 = 4 bytes
    let hnsw_overhead = num_points * m * 8; // pointers + distances
    vector_size + hnsw_overhead
}

// ─── Helpers ────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::{Collection, CollectionConfig};
    use crate::quantization::QuantizationConfig;
    use crate::search::{DistanceMetric, HnswConfig};
    use crate::tiered::TieredStorageConfig;

    fn test_config() -> CollectionConfig {
        CollectionConfig {
            name: "reindex_test".to_string(),
            dimension: 3,
            distance: DistanceMetric::Euclidean,
            hnsw: HnswConfig::default(),
            search_cache_size: 0,
            enable_bm25: false,
            bm25_text_field: "text".to_string(),
            quantization: QuantizationConfig::default(),
            tiered_storage: TieredStorageConfig::default(),
            retention_days: None,
        }
    }

    fn make_point(id: &str, vector: Vec<f32>) -> Point {
        Point::new(id, vector, serde_json::Value::Null).unwrap()
    }

    #[test]
    fn test_reindex_job_lifecycle() {
        let mut job = ReindexJob::new("job-1".into(), "my_collection".into());
        assert_eq!(job.status, ReindexStatus::Queued);
        assert_eq!(job.progress, 0.0);

        job.set_building();
        assert_eq!(job.status, ReindexStatus::Building);

        job.set_progress(0.5);
        assert!((job.progress - 0.5).abs() < f64::EPSILON);

        job.set_swapping();
        assert_eq!(job.status, ReindexStatus::Swapping);

        job.set_completed();
        assert_eq!(job.status, ReindexStatus::Completed);
        assert!((job.progress - 1.0).abs() < f64::EPSILON);
        assert!(job.completed_at.is_some());
    }

    #[test]
    fn test_reindex_job_failure() {
        let mut job = ReindexJob::new("job-2".into(), "test".into());
        job.set_building();
        job.set_failed("out of memory".to_string());

        assert_eq!(job.status, ReindexStatus::Failed);
        assert_eq!(job.error, Some("out of memory".to_string()));
        assert!(job.completed_at.is_some());
    }

    #[test]
    fn test_build_new_index() {
        let config = test_config();
        let points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.0, 0.0, 1.0]),
        ];

        let index = build_new_index(&config, &points).unwrap();
        let results = index.search(&[1.0, 0.0, 0.0], 2, None).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn test_apply_delta_additions() {
        let config = test_config();
        let snapshot_points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
        ];
        let snapshot_ids: HashSet<String> = snapshot_points.iter().map(|p| p.id.clone()).collect();

        let mut new_index = build_new_index(&config, &snapshot_points).unwrap();

        // Current state has an additional point "c"
        let mut current_points = HashMap::new();
        current_points.insert("a".to_string(), make_point("a", vec![1.0, 0.0, 0.0]));
        current_points.insert("b".to_string(), make_point("b", vec![0.0, 1.0, 0.0]));
        current_points.insert("c".to_string(), make_point("c", vec![0.0, 0.0, 1.0]));

        let (added, removed) = apply_delta(&mut new_index, &snapshot_ids, &current_points).unwrap();
        assert_eq!(added, 1);
        assert_eq!(removed, 0);

        // "c" should now be searchable
        let results = new_index.search(&[0.0, 0.0, 1.0], 1, None).unwrap();
        assert_eq!(results[0].0, "c");
    }

    #[test]
    fn test_apply_delta_removals() {
        let config = test_config();
        let snapshot_points = vec![
            make_point("a", vec![1.0, 0.0, 0.0]),
            make_point("b", vec![0.0, 1.0, 0.0]),
            make_point("c", vec![0.0, 0.0, 1.0]),
        ];
        let snapshot_ids: HashSet<String> = snapshot_points.iter().map(|p| p.id.clone()).collect();

        let mut new_index = build_new_index(&config, &snapshot_points).unwrap();

        // Current state has "b" removed
        let mut current_points = HashMap::new();
        current_points.insert("a".to_string(), make_point("a", vec![1.0, 0.0, 0.0]));
        current_points.insert("c".to_string(), make_point("c", vec![0.0, 0.0, 1.0]));

        let (added, removed) = apply_delta(&mut new_index, &snapshot_ids, &current_points).unwrap();
        assert_eq!(added, 0);
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_needs_reindex() {
        // 0 tombstones → no reindex
        assert!(!needs_reindex(0, 100));

        // 10% → no reindex
        assert!(!needs_reindex(10, 100));

        // 20% → no reindex (threshold is >20%)
        assert!(!needs_reindex(20, 100));

        // 21% → reindex
        assert!(needs_reindex(21, 100));

        // 50% → reindex
        assert!(needs_reindex(50, 100));

        // empty collection → no reindex
        assert!(!needs_reindex(0, 0));
    }

    #[test]
    fn test_reindex_cleans_tombstones() {
        let config = test_config();
        let mut col = Collection::new(config.clone());

        // Insert points
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();
        col.insert(make_point("c", vec![0.0, 0.0, 1.0])).unwrap();

        // Remove a point (creates tombstone in the index)
        col.remove("b").unwrap();
        assert!(
            col.tombstone_count() > 0,
            "should have tombstones after remove"
        );

        // Build new index from current points (simulates reindex)
        let current_points: Vec<Point> = col.points_owned();
        let new_index = build_new_index(&config, &current_points).unwrap();

        // Swap the index
        col.swap_index(new_index);

        // After swap, tombstones should be 0
        assert_eq!(
            col.tombstone_count(),
            0,
            "tombstones should be 0 after reindex"
        );

        // Search should still work
        let results = col.search(&[1.0, 0.0, 0.0], 3, None, None).unwrap();
        assert_eq!(results.len(), 2); // a and c
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn test_reindex_concurrent_search() {
        let config = test_config();
        let mut col = Collection::new(config.clone());

        // Insert points
        for i in 0..10 {
            let mut vec = vec![0.0; 3];
            vec[i % 3] = 1.0;
            col.insert(make_point(&format!("p{i}"), vec)).unwrap();
        }

        // Snapshot points for reindex
        let snapshot: Vec<Point> = col.points_owned();

        // Simulate: build new index (this would be in a thread)
        let new_index = build_new_index(&config, &snapshot).unwrap();

        // Meanwhile, search should still work on old index
        let results = col.search(&[1.0, 0.0, 0.0], 5, None, None).unwrap();
        assert!(
            !results.is_empty(),
            "search should work during reindex build"
        );

        // Swap
        col.swap_index(new_index);

        // Search should still work after swap
        let results_after = col.search(&[1.0, 0.0, 0.0], 5, None, None).unwrap();
        assert!(!results_after.is_empty(), "search should work after swap");
    }

    #[test]
    fn test_reindex_with_concurrent_writes() {
        let config = test_config();
        let mut col = Collection::new(config.clone());

        // Insert initial points
        col.insert(make_point("a", vec![1.0, 0.0, 0.0])).unwrap();
        col.insert(make_point("b", vec![0.0, 1.0, 0.0])).unwrap();

        // Take snapshot (phase 1)
        let (snapshot_points, snapshot_ids) = col.points_snapshot();

        // Build new index from snapshot (phase 1 — no lock needed)
        let mut new_index = build_new_index(&config, &snapshot_points).unwrap();

        // Simulate concurrent writes during building:
        col.insert(make_point("c", vec![0.0, 0.0, 1.0])).unwrap(); // addition
        col.remove("b").unwrap(); // removal

        // Phase 2: apply delta and swap
        let current_points_map: HashMap<String, Point> = col
            .points_owned()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        apply_delta(&mut new_index, &snapshot_ids, &current_points_map).unwrap();

        col.swap_index(new_index);

        // Verify: "c" (added during build) should appear
        let results = col.search(&[0.0, 0.0, 1.0], 5, None, None).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.0.as_str()).collect();
        assert!(
            ids.contains(&"c"),
            "point added during reindex should appear after swap"
        );

        // "b" (removed during build) should NOT appear
        let all_results = col.search(&[0.0, 1.0, 0.0], 5, None, None).unwrap();
        let all_ids: Vec<&str> = all_results.iter().map(|r| r.0.as_str()).collect();
        assert!(
            !all_ids.contains(&"b"),
            "point removed during reindex should not appear"
        );

        // "a" should still be there
        assert!(col.get("a").is_some());
    }

    #[test]
    fn test_estimate_index_size() {
        let size = estimate_index_size(1000, 384, 16);
        assert!(size > 0);
        // 1000 * 384 * 4 + 1000 * 16 * 8 = 1_536_000 + 128_000 = 1_664_000
        assert_eq!(size, 1_664_000);
    }

    #[test]
    fn test_reindex_stats_default() {
        let stats = ReindexStats::default();
        assert_eq!(stats.points_processed, 0);
        assert_eq!(stats.points_total, 0);
        assert_eq!(stats.tombstones_cleaned, 0);
    }

    #[test]
    fn test_reindex_job_serialization() {
        let job = ReindexJob::new("job-1".into(), "test_col".into());
        let json = serde_json::to_string(&job).unwrap();
        let deserialized: ReindexJob = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "job-1");
        assert_eq!(deserialized.collection, "test_col");
        assert_eq!(deserialized.status, ReindexStatus::Queued);
    }
}
