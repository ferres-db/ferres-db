# WAL Durability Policy — Design Spec

**Date:** 2026-04-27
**Status:** Approved
**Branch:** fix/005

## Problem

`Wal::append_entry` calls only `BufWriter::flush()`, which moves data to the OS page cache but does NOT call `fsync`/`sync_data()`. This means data can be lost on kernel panic or power loss. The tiered storage module (`tiered.rs`) correctly calls `sync_all()` at 5 points, but the WAL — the primary crash recovery mechanism — has no durability guarantee beyond OS page cache.

## Goal

Make WAL durability explicit, configurable, and safe by default.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| `sync_data()` vs `sync_all()` | `sync_data()` | Faster (skips metadata), sufficient for data durability, works on ext4/xfs/btrfs/NTFS |
| Background thread vs inline | Inline check | No `Arc<Mutex<File>>` contention, simpler, slight latency jitter acceptable |
| Config API | `Wal::open_with_config()` | Backward compat — existing callers keep working |
| Metrics wiring | Callback from core to server | Core crate stays prometheus-free |

## WalConfig Struct

Added to `crates/core/src/wal.rs`:

```rust
use std::time::Duration;

pub struct WalConfig {
    /// Number of ops before triggering automatic snapshot.
    /// Moved from the flat `snapshot_threshold` parameter.
    pub snapshot_threshold: usize,

    /// Enable zstd compression for WAL entries.
    /// Moved from the flat `compress` parameter.
    pub compress: bool,

    /// Call sync_data() after every append. Maximally safe, slower.
    /// Default: false (matches current behavior for backward compat).
    /// Production SHOULD set this to true.
    pub fsync_per_write: bool,

    /// When fsync_per_write=false, maximum time between sync_data() calls.
    /// Default: 1 second.
    pub fsync_interval: Duration,

    /// When fsync_per_write=false, maximum number of ops before forcing sync_data().
    /// Default: 1000 ops.
    pub fsync_every_n_ops: usize,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            snapshot_threshold: 1000,
            compress: false,
            fsync_per_write: false,
            fsync_interval: Duration::from_secs(1),
            fsync_every_n_ops: 1000,
        }
    }
}
```

### Default Rationale

`fsync_per_write: false` is the default for **backward compatibility** (current behavior has no fsync). Production callers (`VectorDB::with_storage_options`) will explicitly set `fsync_per_write: true`. This is documented prominently.

## Wal Struct Changes

New fields on `Wal`:

```rust
pub struct Wal {
    // ... existing fields ...
    config: WalConfig,           // replaces individual params
    ops_since_fsync: usize,     // NEW — counter for periodic fsync
    last_fsync: Instant,        // NEW — timestamp of last fsync
    fsync_hook: Option<Box<dyn Fn(Duration) + Send>>,  // NEW — metrics callback
}
```

### Fsync Hook

To avoid adding prometheus as a dependency of `crates/core`, the WAL exposes an optional callback:

```rust
/// Called after each sync_data() completes.
/// Arg: sync_duration — time the sync_data() call took.
pub fn set_fsync_hook(&mut self, hook: Box<dyn Fn(Duration) + Send>) {
    self.fsync_hook = Some(hook);
}

/// Returns number of ops since last fsync (for metrics gauge).
pub fn ops_since_fsync(&self) -> usize {
    self.ops_since_fsync
}
```

The server crate registers a hook that increments `ferresdb_wal_fsync_total` and observes `ferresdb_wal_fsync_duration_seconds`.

## Fsync Flow in `append_entry`

After the existing `writer.flush()` call:

```rust
// Existing: flush BufWriter to OS page cache
writer.flush().map_err(...)?;

// NEW: durability guarantee
let needs_fsync = if self.config.fsync_per_write {
    true
} else {
    self.ops_since_fsync += 1;
    self.ops_since_fsync >= self.config.fsync_every_n_ops
        || self.last_fsync.elapsed() >= self.config.fsync_interval
};

if needs_fsync {
    let sync_start = Instant::now();
    // Access the underlying File from BufWriter
    self.writer.as_ref().unwrap().get_ref().sync_data()
        .map_err(|e| FerresError::Storage(format!("WAL fsync failed: {e}")))?;
    let sync_duration = sync_start.elapsed();

    self.ops_since_fsync = 0;
    self.last_fsync = Instant::now();

    if let Some(ref hook) = self.fsync_hook {
        hook(sync_duration);
    }
}
```

**Note:** `BufWriter::get_ref()` returns `&File`. `sync_data()` takes `&self`, so this works without `get_mut()`.

## API Changes

### Backward-compatible open

```rust
impl Wal {
    /// Existing API — preserved for backward compat.
    /// Creates WalConfig with default fsync_per_write=false.
    pub fn open(collection_dir: &Path, snapshot_threshold: usize, compress: bool) -> Result<Self, FerresError> {
        let config = WalConfig {
            snapshot_threshold,
            compress,
            ..Default::default()
        };
        Self::open_with_config(collection_dir, config)
    }

    /// New API — accepts full WalConfig.
    pub fn open_with_config(collection_dir: &Path, config: WalConfig) -> Result<Self, FerresError> {
        // ... existing open logic, using config fields ...
    }
}
```

### Caller updates

- `crates/core/src/lib.rs` (line 615): Switch from `Wal::open()` to `Wal::open_with_config()` with `fsync_per_write: true` for production collections. Read `FERRESDB_WAL_FSYNC_PER_WRITE` env var to allow override (default: `true`).
- `crates/server/src/state.rs` (PITR restore): Switch to `Wal::open_with_config()` with the same env var logic.

## Production Default

`VectorDB::with_storage_options` will construct `WalConfig` with `fsync_per_write: true` when creating WALs for new collections. This is the safe default. Users can override via env var `FERRESDB_WAL_FSYNC_PER_WRITE=false` for benchmarks or development.

## Metrics

### New metrics in `crates/server/src/metrics.rs`

```rust
lazy_static! {
    // ... existing metrics ...

    pub static ref WAL_FSYNC_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_wal_fsync_total",
        "Total number of WAL fsync calls",
        &["collection"]
    ).unwrap();

    pub static ref WAL_FSYNC_DURATION: HistogramVec = register_histogram_vec!(
        "ferresdb_wal_fsync_duration_seconds",
        "WAL fsync latency in seconds",
        &["collection"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    ).unwrap();

    pub static ref WAL_PENDING_FSYNC_OPS: GaugeVec = register_gauge_vec!(
        "ferresdb_wal_pending_fsync_ops",
        "Number of WAL ops since last fsync",
        &["collection"]
    ).unwrap();
}
```

### Hook registration

When `VectorDB` creates a WAL in production mode, it registers the fsync hook:

```rust
let collection_name = name.clone();
wal.set_fsync_hook(Box::new(move |duration| {
    WAL_FSYNC_TOTAL.with_label_values(&[&collection_name]).inc();
    WAL_FSYNC_DURATION.with_label_values(&[&collection_name]).observe(duration.as_secs_f64());
}));
```

`WAL_PENDING_FSYNC_OPS` is updated by the server's metrics endpoint by reading `wal.ops_since_fsync` via a public getter.

## Tests

### New tests in `crates/core/src/wal.rs`

1. **`wal_config_default_no_fsync`** — verify `WalConfig::default()` has `fsync_per_write=false`, matching current behavior.

2. **`wal_fsync_per_write_durability`** — create WAL with `fsync_per_write=true`, write 3 entries, drop WAL (flushes + syncs), reopen, read entries back. Verify all 3 present.

3. **`wal_no_fsync_still_flushes_to_os`** — create WAL with `fsync_per_write=false`, write entries, call `flush()`. Data is in OS page cache and readable. Documents the trade-off: data survives process crash but not kernel panic.

4. **`wal_periodic_fsync_after_n_ops`** — set `fsync_every_n_ops=5`, write 5 entries, verify that a sync happened (via hook counter or by checking ops_since_fsync reset).

5. **`wal_periodic_fsync_after_interval`** — set `fsync_interval=1ms`, write 1 entry, sleep 2ms, write another, verify sync triggered.

6. **`wal_open_with_config_backward_compat`** — verify `Wal::open()` produces same behavior as `Wal::open_with_config()` with default config.

### Existing tests

All 22 existing tests continue to pass — `Wal::open()` preserves the current API and default behavior (no fsync).

## Benchmark Updates

### `crates/benchmark/src/main.rs`

- Add `--fsync-per-write` CLI flag (via clap).
- Pass flag to server as `FERRESDB_WAL_FSYNC_PER_WRITE=true` env var when starting.
- In `print_markdown_report()`, add row: `| WAL fsync | per-write / periodic / off |`.
- Document: "Run with `--fsync-per-write` to measure durability cost. Compare throughput with and without."

### `crates/core/benches/performance.rs`

- Add criterion benchmark `benchmark_wal_fsync_overhead`: upsert 10k points with fsync_per_write=true vs false, report the delta.

## Architecture Docs Update

Add to `docs/architecture.md`:

```markdown
## Durability and fsync semantics

The Write-Ahead Log (WAL) guarantees durability through `sync_data()` calls.
Two modes are available:

- **Per-write fsync** (`fsync_per_write=true`, production default): Every
  `append_*` call returns `Ok` only after data is persisted to disk. Safe
  against kernel panic and power loss. ~10x slower than no-fsync on HDD,
  ~2-3x on SSD.

- **Periodic fsync** (`fsync_per_write=false`): Data is flushed to OS page
  cache on every write but only synced to disk every `fsync_interval` (1s
  default) or `fsync_every_n_ops` (1000 default). Data survives process
  crash but may be lost on kernel panic or power loss.

The WAL uses `sync_data()` (not `sync_all()`) — this flushes file content
without metadata, which is faster and sufficient for crash recovery.
```

## Files Changed

| File | Change |
|------|--------|
| `crates/core/src/wal.rs` | Add `WalConfig`, fsync logic, new tests, module docstring |
| `crates/core/src/lib.rs` | Use `WalConfig` with `fsync_per_write=true` in production path |
| `crates/server/src/metrics.rs` | Add 3 WAL metrics |
| `crates/server/src/state.rs` | Register fsync hook on WAL creation |
| `crates/benchmark/src/main.rs` | Add `--fsync-per-write` flag, report in results |
| `docs/architecture.md` | Add durability section |
