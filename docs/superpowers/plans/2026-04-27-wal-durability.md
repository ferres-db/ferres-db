# WAL Durability Policy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make WAL durability explicit and configurable — `sync_data()` per write or periodic, with `fsync_per_write=true` as production default.

**Architecture:** Inline fsync check inside `append_entry()` (no background thread). New `WalConfig` struct replaces flat parameters. Callback hook for Prometheus metrics from core→server. `Wal::open_with_config()` added alongside existing `Wal::open()` for backward compat.

**Tech Stack:** Rust, `std::fs::File::sync_data()`, `std::time::Instant`, Prometheus (server crate only), criterion benchmarks

**Spec:** `docs/superpowers/specs/2026-04-27-wal-durability-design.md`

---

### Task 1: Add WalConfig struct and Wal::open_with_config()

**Files:**
- Modify: `crates/core/src/wal.rs:24-26` (imports), `crates/core/src/wal.rs:82-93` (Wal struct), `crates/core/src/wal.rs:99-159` (open method)

- [ ] **Step 1: Write the failing test for WalConfig::default()**

Add to the test module in `crates/core/src/wal.rs` (after line 826):

```rust
#[test]
fn wal_config_default_no_fsync() {
    let cfg = WalConfig::default();
    assert_eq!(cfg.snapshot_threshold, 1000);
    assert!(!cfg.compress);
    assert!(!cfg.fsync_per_write);
    assert_eq!(cfg.fsync_interval, Duration::from_secs(1));
    assert_eq!(cfg.fsync_every_n_ops, 1000);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ferres-db-core wal_config_default_no_fsync`
Expected: FAIL — `WalConfig` not found

- [ ] **Step 3: Add WalConfig struct and update Wal**

Add `use std::time::{Duration, Instant};` to imports (line 24 area).

Add before the `Wal` struct definition (before line 82):

```rust
/// Configuração do Write-Ahead Log.
///
/// O default (`fsync_per_write=false`) mantém compatibilidade com o comportamento
/// anterior (sem fsync). Em produção, USE `fsync_per_write=true`.
pub struct WalConfig {
    /// Número de operações antes de disparar snapshot automático.
    pub snapshot_threshold: usize,
    /// Habilita compressão Zstd para entradas do WAL.
    pub compress: bool,
    /// Chama `sync_data()` após cada `append_*`. Mais seguro, mais lento.
    /// Default: false (compat). Produção DEVE usar true.
    pub fsync_per_write: bool,
    /// Quando `fsync_per_write=false`, intervalo máximo entre sync_data().
    pub fsync_interval: Duration,
    /// Quando `fsync_per_write=false`, número máximo de ops antes de forçar sync_data().
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

Update `Wal` struct to add new fields:

```rust
pub struct Wal {
    collection_dir: PathBuf,
    writer: Option<BufWriter<fs::File>>,
    ops_since_snapshot: usize,
    config: WalConfig,
    ops_since_fsync: usize,
    last_fsync: Instant,
    fsync_hook: Option<Box<dyn Fn(Duration) + Send>>,
}
```

Remove the old `snapshot_threshold` and `compress` fields — they're now in `config`.

- [ ] **Step 4: Add Wal::open_with_config() and refactor Wal::open()**

Replace the existing `open` method with:

```rust
/// Abre (ou cria) o WAL com configuração padrão (sem fsync).
pub fn open(
    collection_dir: &Path,
    snapshot_threshold: usize,
    compress: bool,
) -> Result<Self, FerresError> {
    let config = WalConfig {
        snapshot_threshold,
        compress,
        ..Default::default()
    };
    Self::open_with_config(collection_dir, config)
}

/// Abre (ou cria) o WAL com configuração completa.
pub fn open_with_config(
    collection_dir: &Path,
    config: WalConfig,
) -> Result<Self, FerresError> {
    fs::create_dir_all(collection_dir).map_err(|e| {
        FerresError::Storage(format!(
            "failed to create collection directory {}: {e}",
            collection_dir.display()
        ))
    })?;

    let wal_path = collection_dir.join("wal.log");

    let ops_since_snapshot = if wal_path.exists() {
        Self::count_entries(&wal_path)?
    } else {
        0
    };

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&wal_path)
        .map_err(|e| {
            FerresError::Storage(format!("failed to open WAL at {}: {e}", wal_path.display()))
        })?;

    if config.compress {
        let meta = file.metadata().map_err(|e| {
            FerresError::Storage(format!("failed to stat WAL at {}: {e}", wal_path.display()))
        })?;
        if meta.len() == 0 {
            file.write_all(WAL_ZSTD_MAGIC).map_err(|e| {
                FerresError::Storage(format!(
                    "failed to write WAL magic at {}: {e}",
                    wal_path.display()
                ))
            })?;
        }
    }

    Ok(Self {
        collection_dir: collection_dir.to_path_buf(),
        writer: Some(BufWriter::new(file)),
        ops_since_snapshot,
        config,
        ops_since_fsync: 0,
        last_fsync: Instant::now(),
        fsync_hook: None,
    })
}
```

Update all internal references from `self.snapshot_threshold` → `self.config.snapshot_threshold` and `self.compress` → `self.config.compress`.

- [ ] **Step 5: Add public getters**

```rust
/// Número de operações desde o último fsync (para métricas).
pub fn ops_since_fsync(&self) -> usize {
    self.ops_since_fsync
}

/// Registra um callback chamado após cada sync_data().
pub fn set_fsync_hook(&mut self, hook: Box<dyn Fn(Duration) + Send>) {
    self.fsync_hook = Some(hook);
}
```

- [ ] **Step 6: Run the test**

Run: `cargo test -p ferres-db-core wal_config_default_no_fsync`
Expected: PASS

- [ ] **Step 7: Run all existing WAL tests to verify no regressions**

Run: `cargo test -p ferres-db-core wal_`
Expected: All 22 existing tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/wal.rs
git commit -m "feat(wal): add WalConfig struct and open_with_config() API"
```

---

### Task 2: Implement inline fsync in append_entry

**Files:**
- Modify: `crates/core/src/wal.rs:354-388` (append_entry method)

- [ ] **Step 1: Write the failing test for fsync_per_write durability**

```rust
#[test]
fn wal_fsync_per_write_durability() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("fsync_col");

    let config = WalConfig {
        fsync_per_write: true,
        ..Default::default()
    };

    // Write entries with fsync enabled
    {
        let mut wal = Wal::open_with_config(&dir, config).unwrap();
        wal.append_upsert(&make_point("p1", vec![1.0, 2.0, 3.0])).unwrap();
        wal.append_upsert(&make_point("p2", vec![4.0, 5.0, 6.0])).unwrap();
        wal.append_upsert(&make_point("p3", vec![7.0, 8.0, 9.0])).unwrap();
        // Wal is dropped here — entries should be on disk
    }

    // Reopen and verify all entries present
    let entries = Wal::read_entries(&dir).unwrap();
    assert_eq!(entries.len(), 3);
    match &entries[2].operation {
        WalOperation::Upsert { point } => assert_eq!(point.id, "p3"),
        _ => panic!("expected upsert"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ferres-db-core wal_fsync_per_write_durability`
Expected: FAIL — fsync logic not implemented yet (test may actually pass since data is flushed, but the fsync code path isn't exercised)

- [ ] **Step 3: Write the failing test for periodic fsync by op count**

```rust
#[test]
fn wal_periodic_fsync_after_n_ops() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("periodic_col");

    let sync_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sync_count_clone = sync_count.clone();

    let config = WalConfig {
        fsync_per_write: false,
        fsync_every_n_ops: 5,
        fsync_interval: Duration::from_secs(3600), // high — won't trigger
        ..Default::default()
    };

    let mut wal = Wal::open_with_config(&dir, config).unwrap();
    wal.set_fsync_hook(Box::new(move |_dur| {
        sync_count_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));

    // Write 5 entries — should trigger exactly 1 fsync
    for i in 0..5 {
        wal.append_upsert(&make_point(&format!("p{i}"), vec![i as f32])).unwrap();
    }

    assert_eq!(sync_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(wal.ops_since_fsync(), 0); // reset after fsync
}
```

- [ ] **Step 4: Write the failing test for periodic fsync by interval**

```rust
#[test]
fn wal_periodic_fsync_after_interval() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("interval_col");

    let sync_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sync_count_clone = sync_count.clone();

    let config = WalConfig {
        fsync_per_write: false,
        fsync_every_n_ops: 10000, // high — won't trigger by count
        fsync_interval: Duration::from_millis(1),
        ..Default::default()
    };

    let mut wal = Wal::open_with_config(&dir, config).unwrap();
    wal.set_fsync_hook(Box::new(move |_dur| {
        sync_count_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));

    wal.append_upsert(&make_point("p1", vec![1.0])).unwrap();
    assert_eq!(sync_count.load(std::sync::atomic::Ordering::Relaxed), 0);

    std::thread::sleep(Duration::from_millis(5));

    wal.append_upsert(&make_point("p2", vec![2.0])).unwrap();
    assert_eq!(sync_count.load(std::sync::atomic::Ordering::Relaxed), 1);
}
```

- [ ] **Step 5: Implement fsync logic in append_entry**

Replace the end of `append_entry` (after line 383's `writer.flush()`) with:

```rust
writer
    .flush()
    .map_err(|e| FerresError::Storage(format!("failed to flush WAL: {e}")))?;

// Durability: sync_data() to persist to disk
let needs_fsync = if self.config.fsync_per_write {
    true
} else {
    self.ops_since_fsync += 1;
    self.ops_since_fsync >= self.config.fsync_every_n_ops
        || self.last_fsync.elapsed() >= self.config.fsync_interval
};

if needs_fsync {
    let sync_start = Instant::now();
    self.writer
        .as_ref()
        .unwrap()
        .get_ref()
        .sync_data()
        .map_err(|e| FerresError::Storage(format!("WAL fsync failed: {e}")))?;
    let sync_duration = sync_start.elapsed();

    self.ops_since_fsync = 0;
    self.last_fsync = Instant::now();

    if let Some(ref hook) = self.fsync_hook {
        hook(sync_duration);
    }
}

Ok(())
```

- [ ] **Step 6: Run all three new tests**

Run: `cargo test -p ferres-db-core wal_fsync_per_write_durability wal_periodic_fsync_after_n_ops wal_periodic_fsync_after_interval`
Expected: All PASS

- [ ] **Step 7: Run all existing WAL tests to verify no regressions**

Run: `cargo test -p ferres-db-core wal_`
Expected: All 22+ tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/wal.rs
git commit -m "feat(wal): implement inline fsync with per-write and periodic modes"
```

---

### Task 3: Update module docstring with durability semantics

**Files:**
- Modify: `crates/core/src/wal.rs:1-23` (module docstring)

- [ ] **Step 1: Update the module docstring**

Replace the module docstring (lines 1-23) with:

```rust
//! # Write-Ahead Log (WAL) — durabilidade de operações
//!
//! Antes de modificar uma coleção em memória, a operação é registrada
//! no WAL (`wal.log`). Em caso de crash, as operações pendentes são
//! re-aplicadas sobre o último snapshot na recuperação.
//!
//! ## Durability semantics
//!
//! `Wal::append_*` retorna `Ok` somente após:
//! - `write_all` ter sucedido (dados no buffer do OS);
//! - `flush()` ter descarregado o BufWriter para o page cache do OS;
//! - `sync_data()` ter persistido os dados em disco (quando configurado).
//!
//! Se `fsync_per_write = true` (padrão em produção), cada `append_*` chama
//! `sync_data()` antes de retornar `Ok`. Dados são duráveis contra kernel
//! panic e perda de energia.
//!
//! Se `fsync_per_write = false`, o `sync_data()` é chamado periodicamente
//! (a cada `fsync_interval` ou `fsync_every_n_ops` operações). Dados podem
//! ser perdidos em kernel panic ou perda de energia entre o `write` e o
//! próximo fsync.
//!
//! O WAL usa `sync_data()` (não `sync_all()`) — flush de conteúdo sem
//! metadados, mais rápido e suficiente para recuperação de crash.
//!
//! ## Formato
//!
//! O WAL usa JSON-lines — cada linha é um `WalEntry` serializado:
//! ```json
//! {"timestamp":1234567890,"operation":{"op":"upsert","point":{...}}}
//! {"timestamp":1234567891,"operation":{"op":"delete","id":"point-123"}}
//! ```
//!
//! Com **compressão opcional** (Zstd), o ficheiro começa com o magic `WALz` e cada
//! entrada é armazenada como frame: 4 bytes (u32 LE) tamanho + payload comprimido.
//!
//! ## Snapshot
//!
//! A cada `snapshot_threshold` operações (padrão: 1000), um snapshot
//! completo é criado via `FileStorage::save_collection` e o WAL é
//! truncado.
```

- [ ] **Step 2: Verify doc compiles**

Run: `cargo doc -p ferres-db-core --no-deps`
Expected: No warnings

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/wal.rs
git commit -m "docs(wal): add durability semantics to module docstring"
```

---

### Task 4: Add WAL metrics to server crate

**Files:**
- Modify: `crates/server/src/metrics.rs:11-79` (lazy_static block)

- [ ] **Step 1: Add WAL metrics to metrics.rs**

Add to the `lazy_static!` block (after the LLM Proxy section, before the closing `}`):

```rust
// ─── WAL Durability Metrics ────────────────────────────────────────────

/// Total de chamadas sync_data() no WAL, por coleção.
pub static ref WAL_FSYNC_TOTAL: CounterVec = register_counter_vec!(
    "ferresdb_wal_fsync_total",
    "Total number of WAL fsync calls",
    &["collection"]
).unwrap();

/// Latência das chamadas sync_data() no WAL, em segundos.
pub static ref WAL_FSYNC_DURATION: HistogramVec = register_histogram_vec!(
    "ferresdb_wal_fsync_duration_seconds",
    "WAL fsync latency in seconds",
    &["collection"],
    vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
).unwrap();

/// Número de operações WAL desde o último fsync.
pub static ref WAL_PENDING_FSYNC_OPS: GaugeVec = register_gauge_vec!(
    "ferresdb_wal_pending_fsync_ops",
    "Number of WAL ops since last fsync",
    &["collection"]
).unwrap();
```

Add `GaugeVec` to the import on line 8:

```rust
use prometheus::{
    register_counter_vec, register_gauge, register_gauge_vec, register_histogram_vec,
    CounterVec, Gauge, GaugeVec, HistogramVec, TextEncoder,
};
```

- [ ] **Step 2: Verify server crate compiles**

Run: `cargo check -p ferres-db-server`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/metrics.rs
git commit -m "feat(metrics): add WAL fsync Prometheus metrics"
```

---

### Task 5: Wire fsync hook in VectorDB and update production default

**Files:**
- Modify: `crates/core/src/lib.rs:612-620` (WAL creation in create_collection)
- Modify: `crates/core/src/lib.rs:486` (VectorDB struct — add fsync_per_write field)
- Modify: `crates/core/src/storage.rs` (StorageOptions — add wal_fsync_per_write)

- [ ] **Step 1: Add wal_fsync_per_write to StorageOptions**

In `crates/core/src/storage.rs`, add to `StorageOptions`:

```rust
/// Faz fsync após cada append do WAL. Default: true em produção.
pub wal_fsync_per_write: bool,
```

Update the `Default` impl for `StorageOptions` to set `wal_fsync_per_write: true`.

- [ ] **Step 2: Update WAL creation to use WalConfig with production default**

In `crates/core/src/lib.rs`, replace the WAL creation (line 615 area):

```rust
// Abre WAL para a nova coleção
let wal_config = wal::WalConfig {
    snapshot_threshold: wal::Wal::DEFAULT_SNAPSHOT_THRESHOLD,
    compress: self.storage_options.wal_compression,
    fsync_per_write: self.storage_options.wal_fsync_per_write,
    ..Default::default()
};
let wal_handle = wal::Wal::open_with_config(&collection_dir, wal_config)?;
self.wals.insert(name.clone(), wal_handle);
```

- [ ] **Step 3: Verify core crate compiles**

Run: `cargo check -p ferres-db-core`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/lib.rs crates/core/src/storage.rs
git commit -m "feat(wal): use WalConfig with fsync_per_write=true in production"
```

---

### Task 6: Register fsync hook from server crate

**Files:**
- Modify: `crates/server/src/state.rs` (WAL creation / VectorDB initialization)

- [ ] **Step 1: Find where VectorDB is created in the server**

In `crates/server/src/state.rs`, find the `VectorDB::with_storage_options` call and the point where WALs are available.

- [ ] **Step 2: Add hook registration after WAL creation**

After WALs are created (or via a new method on VectorDB that accepts a hook factory), register the Prometheus hook. Since WALs are created per-collection inside `create_collection`, the cleanest approach is a method on VectorDB:

Add to `crates/core/src/lib.rs`:

```rust
/// Registra um hook de fsync para uma coleção específica.
/// O hook é chamado após cada sync_data() do WAL.
pub fn set_wal_fsync_hook(
    &mut self,
    collection: &str,
    hook: Box<dyn Fn(Duration) + Send>,
) -> Result<(), FerresError> {
    let wal = self.wals.get_mut(collection)
        .ok_or_else(|| FerresError::CollectionNotFound(collection.to_string()))?;
    wal.set_fsync_hook(hook);
    Ok(())
}
```

In the server crate, after creating or loading collections, register the hook:

```rust
use crate::metrics::{WAL_FSYNC_TOTAL, WAL_FSYNC_DURATION};

// After collections are loaded:
for name in db.collection_names() {
    let collection_name = name.clone();
    db.set_wal_fsync_hook(&name, Box::new(move |duration| {
        WAL_FSYNC_TOTAL.with_label_values(&[&collection_name]).inc();
        WAL_FSYNC_DURATION.with_label_values(&[&collection_name]).observe(duration.as_secs_f64());
    }))?;
}
```

- [ ] **Step 3: Verify server crate compiles**

Run: `cargo check -p ferres-db-server`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/lib.rs crates/server/src/state.rs
git commit -m "feat(server): register WAL fsync hook for Prometheus metrics"
```

---

### Task 7: Add the no-fsync trade-off documentation test

**Files:**
- Modify: `crates/core/src/wal.rs` (test module)

- [ ] **Step 1: Add the trade-off documentation test**

```rust
#[test]
fn wal_no_fsync_still_flushes_to_os() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("nofsync_col");

    let config = WalConfig {
        fsync_per_write: false,
        fsync_every_n_ops: 10000,
        fsync_interval: Duration::from_secs(3600),
        ..Default::default()
    };

    {
        let mut wal = Wal::open_with_config(&dir, config).unwrap();
        wal.append_upsert(&make_point("p1", vec![1.0, 2.0, 3.0])).unwrap();
        wal.append_upsert(&make_point("p2", vec![4.0, 5.0, 6.0])).unwrap();
        // BufWriter::flush() is called by append_entry, so data is in OS page cache
    }

    // Data is readable (it's in the OS page cache, flushed by BufWriter)
    let entries = Wal::read_entries(&dir).unwrap();
    assert_eq!(entries.len(), 2);

    // NOTE: This test documents the trade-off. In a real kernel panic,
    // data in the OS page cache but not on disk would be lost.
    // With fsync_per_write=false, we trade durability for write throughput.
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p ferres-db-core wal_no_fsync_still_flushes_to_os`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/wal.rs
git commit -m "test(wal): document no-fsync trade-off with explicit test"
```

---

### Task 8: Update architecture docs

**Files:**
- Modify: `docs/architecture.md` (add durability section)

- [ ] **Step 1: Add durability section to architecture.md**

Append before the "Detailed Documentation" section (before line 84):

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

Configuration is done via `WalConfig` in `crates/core/src/wal.rs` or
the `FERRESDB_WAL_FSYNC_PER_WRITE` environment variable.
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: add WAL durability and fsync semantics section"
```

---

### Task 9: Update benchmark to report fsync configuration

**Files:**
- Modify: `crates/benchmark/src/main.rs` (CLI flags and report)

- [ ] **Step 1: Add --fsync-per-write CLI flag**

In the clap CLI definition, add:

```rust
/// Enable WAL fsync per write (durable but slower)
#[arg(long, default_value_t = false)]
fsync_per_write: bool,
```

- [ ] **Step 2: Pass flag to server env**

Where the server process is started, add the env var:

```rust
if args.fsync_per_write {
    cmd.env("FERRESDB_WAL_FSYNC_PER_WRITE", "true");
}
```

- [ ] **Step 3: Add durability row to markdown report**

In `print_markdown_report()`, add:

```rust
let fsync_mode = if self.fsync_per_write { "per-write" } else { "off" };
println!("| WAL fsync | {} |", fsync_mode);
```

- [ ] **Step 4: Verify benchmark compiles**

Run: `cargo check -p ferres-db-benchmark`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add crates/benchmark/src/main.rs
git commit -m "feat(benchmark): add --fsync-per-write flag and report in results"
```

---

### Task 10: Update valt/ with changes

**Files:**
- Modify: `valt/decisions.md` (add ADR for WAL durability)
- Modify: `valt/overview.md` (update status block)
- Modify: `valt/index.md` (update counts)

- [ ] **Step 1: Add ADR to valt/decisions.md**

Append to `valt/decisions.md`:

```markdown
## [2026-04-27] ADR-021: WAL durability policy — fsync per write by default
**Status:** ✅ Decided
**Inferred from:** `crates/core/src/wal.rs`, spec at `docs/superpowers/specs/2026-04-27-wal-durability-design.md`

**Context:** The WAL only called `BufWriter::flush()` — data landed in OS page cache but was never synced to disk. A kernel panic or power loss could lose recent writes. The tiered storage module already used `sync_all()`, but the WAL (the primary crash recovery mechanism) had no durability guarantee.

**Decision:** Added `WalConfig` with `fsync_per_write` flag (default: `true` in production). Uses `sync_data()` (not `sync_all()`) for speed. When `fsync_per_write=false`, periodic fsync by op count or time interval. Inline check in `append_entry()` — no background thread.

**Why not the alternatives:**
- **Background fsync thread:** Rejected — adds `Arc<Mutex<File>>` contention on every write, more complex. Inline check has slight latency jitter but no lock overhead.
- **`sync_all()` instead of `sync_data()`:** Rejected — `sync_data()` is faster (skips metadata flush) and sufficient for data durability on all common filesystems.

**Consequences:** Every `append_*` call is ~2-3x slower on SSD, ~10x on HDD. Production data is now durable against kernel panic. Can be disabled for benchmarks via `FERRESDB_WAL_FSYNC_PER_WRITE=false`.
```

- [ ] **Step 2: Update valt/overview.md status block**

Add to the status block in `valt/overview.md`:

```markdown
- [x] WAL durability: fsync_per_write added, production default=true (2026-04-27)
```

- [ ] **Step 3: Update valt/index.md counts**

Update the counts line at the bottom of `valt/index.md` to reflect the new ADR count (21 → 22).

- [ ] **Step 4: Commit**

```bash
git add valt/decisions.md valt/overview.md valt/index.md
git commit -m "valt: document WAL durability decision (ADR-021)"
```

---

### Task 11: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p ferres-db-core`
Expected: All tests PASS (22 existing + 5 new = 27+)

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy -p ferres-db-core -p ferres-db-server`
Expected: No warnings

- [ ] **Step 3: Run cargo fmt check**

Run: `cargo fmt --check`
Expected: No diffs

- [ ] **Step 4: Verify all commits are clean**

Run: `git log --oneline main..HEAD`
Expected: ~10 commits, each focused and well-described
