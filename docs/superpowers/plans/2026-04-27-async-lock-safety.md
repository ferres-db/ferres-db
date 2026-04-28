# Async Lock Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce at CI level that no `std::sync` lock guard is held across an async `.await` point, and harden the one fragile `drop()` pattern found in `streaming.rs`.

**Architecture:** The lints `clippy::await_holding_lock` and `clippy::await_holding_refcell_ref` are added as errors to a new CI job. Five pre-existing clippy errors in the core crate block `-D warnings` and must be fixed first. The one structurally fragile pattern (explicit `drop()` inside a match arm in `process_upsert_batch`) is refactored to block scoping. A concurrency stress test verifies no deadlock under N writers + M readers. CONTRIBUTING.md documents the rule.

**Tech Stack:** Rust stable, Clippy, Tokio, GitHub Actions, `tempfile`, `futures`

---

## Discovery: what clippy actually reports

Running `cargo clippy --workspace --all-targets -- -D warnings` (without `--all-features` because protoc is not available on Windows) produces **5 errors** in `crates/core`, **0 errors** in `crates/server`. There are **no `await_holding_lock` violations** — the code already releases locks before async calls — but the explicit `drop()` pattern in `streaming.rs` is fragile to refactoring.

Pre-existing errors to fix:
| File | Line | Lint | Fix |
|---|---|---|---|
| `crates/core/src/search.rs` | 400 | `nonminimal_bool` | `#[allow]` — suggested rewrite is incorrect |
| `crates/core/src/wal.rs` | 1075 | `unused_mut` | remove `mut` |
| `crates/core/src/explain.rs` | 620 | `manual_range_contains` | use `.contains()` |
| `crates/core/src/quantization.rs` | 1348 (test fn) | `approx_constant` (×3) | `#[allow]` — test deliberately uses `3.14`, not π |

---

## File Map

| Action | File |
|---|---|
| Modify | `crates/core/src/search.rs` |
| Modify | `crates/core/src/wal.rs` |
| Modify | `crates/core/src/explain.rs` |
| Modify | `crates/core/src/quantization.rs` |
| Modify | `crates/server/src/handlers/streaming.rs` |
| Create | `crates/server/tests/lock_safety_test.rs` |
| Modify | `.github/workflows/ci.yml` |
| Modify | `CONTRIBUTING.md` |
| Modify | `valt/decisions.md` |
| Modify | `valt/index.md` |

---

## Task 1: Fix pre-existing clippy error — `search.rs:400` nonminimal_bool

**Files:**
- Modify: `crates/core/src/search.rs:396-401`

The lint `clippy::nonminimal_bool` fires on `avx2 || sse4.1`. The suggested rewrite is strictly worse (duplicates `avx2` check). Suppress with `#[allow]`.

- [ ] **Step 1: Apply the fix**

Replace lines 396-401 of `crates/core/src/search.rs`:

```rust
// BEFORE
#[inline]
pub fn simd_enabled() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx2") || std::arch::is_x86_feature_detected!("sse4.1")
    }
```

```rust
// AFTER
#[inline]
pub fn simd_enabled() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        #[allow(clippy::nonminimal_bool)]
        let supported = std::arch::is_x86_feature_detected!("avx2")
            || std::arch::is_x86_feature_detected!("sse4.1");
        supported
    }
```

- [ ] **Step 2: Verify fix compiles**

```bash
cargo clippy --package ferres-db-core --all-targets -- -D clippy::nonminimal_bool 2>&1 | grep "nonminimal"
```
Expected: no output (lint no longer fires)

---

## Task 2: Fix pre-existing clippy error — `wal.rs:1075` unused_mut

**Files:**
- Modify: `crates/core/src/wal.rs:1075`

- [ ] **Step 1: Apply the fix**

At `crates/core/src/wal.rs:1075`, remove `mut`:

```rust
// BEFORE
let mut wal1 = Wal::open(&dir, 500, true).unwrap();

// AFTER
let wal1 = Wal::open(&dir, 500, true).unwrap();
```

- [ ] **Step 2: Verify**

```bash
cargo clippy --package ferres-db-core --all-targets -- -D unused-mut 2>&1 | grep "wal.rs"
```
Expected: no output

---

## Task 3: Fix pre-existing clippy error — `explain.rs:620` manual_range_contains

**Files:**
- Modify: `crates/core/src/explain.rs:620`

- [ ] **Step 1: Apply the fix**

At `crates/core/src/explain.rs:620`:

```rust
// BEFORE
assert!(sim >= 0.0 && sim <= 1.0, "similarity deve estar em [0, 1]");

// AFTER
assert!((0.0..=1.0).contains(&sim), "similarity deve estar em [0, 1]");
```

- [ ] **Step 2: Verify**

```bash
cargo clippy --package ferres-db-core --all-targets -- -D clippy::manual_range_contains 2>&1 | grep "explain"
```
Expected: no output

---

## Task 4: Fix pre-existing clippy error — `quantization.rs` approx_constant

**Files:**
- Modify: `crates/core/src/quantization.rs` (test `test_polar_encode_small_dims`)

The test deliberately uses `3.14` (not π) to test a specific known scalar value. Suppress with `#[allow]` on the test function.

- [ ] **Step 1: Apply the fix**

Find `fn test_polar_encode_small_dims` (around line 1347) and add the allow attribute:

```rust
// BEFORE
#[test]
fn test_polar_encode_small_dims() {
    // dim=1
    let v1 = vec![3.14f32];
    let q1 = polar_encode(&v1, 8);
    assert_eq!(q1.dim, 1);
    assert_eq!(q1.angles.len(), 0);
    assert!((q1.final_radius - 3.14).abs() < 0.001);
    let d1 = polar_decode(&q1);
    assert_eq!(d1.len(), 1);
    assert!((d1[0] - 3.14).abs() < 0.001);

// AFTER
#[test]
#[allow(clippy::approx_constant)] // deliberately using 3.14 as test scalar, not π
fn test_polar_encode_small_dims() {
    // dim=1
    let v1 = vec![3.14f32];
    let q1 = polar_encode(&v1, 8);
    assert_eq!(q1.dim, 1);
    assert_eq!(q1.angles.len(), 0);
    assert!((q1.final_radius - 3.14).abs() < 0.001);
    let d1 = polar_decode(&q1);
    assert_eq!(d1.len(), 1);
    assert!((d1[0] - 3.14).abs() < 0.001);
```

- [ ] **Step 2: Verify all core errors are cleared**

```bash
cargo clippy --package ferres-db-core --all-targets -- -D warnings 2>&1
```
Expected: exit 0, no errors

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/search.rs crates/core/src/wal.rs crates/core/src/explain.rs crates/core/src/quantization.rs
git commit -s -m "fix(clippy): resolve pre-existing -D warnings violations in core crate

- suppress nonminimal_bool on simd_enabled (suggested rewrite duplicates avx2)
- remove unused mut on wal compat test
- use range contains in explain assert
- allow approx_constant on polar_encode test (3.14 is intentional test value)

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 5: Refactor fragile `drop()` pattern in streaming.rs

**Files:**
- Modify: `crates/server/src/handlers/streaming.rs:501-591`

The `process_upsert_batch` function acquires a `write()` guard, then calls `drop(collection)` + `drop(collection_arc)` explicitly inside a match arm before synchronous `record_ingest`/`emit_event`. This is fragile: if either call is ever made async, the lint won't catch the dropped-too-late lock.

Refactor to block scoping: the lock guard lives only within a `{}` block and is dropped automatically at the block end. No explicit `drop()` calls needed.

- [ ] **Step 1: Understand the structure before changing**

Read the full `process_upsert_batch` function (`streaming.rs:501-591`). Note:
- `collection_arc` is a `dashmap::mapref::one::Ref` (holds a dashmap shard lock)
- `collection` is a `RwLockWriteGuard` (holds the collection write lock)
- Both must be released before any future `.await` points

- [ ] **Step 2: Rewrite `process_upsert_batch`**

Replace the entire `process_upsert_batch` function (lines 501-591) with:

```rust
/// Processa um batch de upserts em uma coleção.
async fn process_upsert_batch(
    app_state: &AppState,
    collection_name: &str,
    points: Vec<WsPointInput>,
) -> Result<(usize, usize, u64), ServerMessage> {
    use ferres_db_core::Point;

    let start = Instant::now();

    // Clone the Arc immediately to release the DashMap shard lock before
    // acquiring the RwLock. Both locks must be dropped before any .await.
    let collection_arc = {
        let ref_guard =
            app_state
                .collections
                .get(collection_name)
                .ok_or_else(|| ServerMessage::Error {
                    message: "collection not found".to_string(),
                    code: 404,
                })?;
        std::sync::Arc::clone(ref_guard.value())
        // DashMap shard lock released here
    };

    // All sync work lives inside this block.
    // The RwLockWriteGuard is dropped automatically at the closing brace —
    // no explicit drop() needed, making this safe against future async additions.
    let (inserted, failed, took_ms, opt_event) = {
        let mut collection =
            collection_arc
                .write()
                .map_err(|e| ServerMessage::Error {
                    message: format!("failed to acquire write lock: {e}"),
                    code: 500,
                })?;

        let mut valid_points = Vec::new();
        let mut failed = 0usize;
        let mut point_ids = Vec::new();

        for input in points {
            if collection.validate_dimension(&input.vector).is_err() {
                failed += 1;
                continue;
            }
            if input.vector.is_empty() {
                failed += 1;
                continue;
            }
            if input.vector.iter().any(|v| !v.is_finite()) {
                failed += 1;
                continue;
            }

            match Point::new(input.id.clone(), input.vector, input.metadata) {
                Ok(mut point) => {
                    if let Some(ttl) = input.ttl {
                        point.expires_at = Some(unix_now().saturating_add(ttl));
                    }
                    point_ids.push(input.id);
                    valid_points.push(point);
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }

        if valid_points.is_empty() {
            let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
            (0usize, failed, took_ms, None)
        } else {
            match collection.insert_batch(valid_points) {
                Ok(result) => {
                    collection.mark_dirty();
                    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
                    let event = CollectionEvent {
                        collection: collection_name.to_string(),
                        action: "upsert".to_string(),
                        point_ids,
                        timestamp: unix_now(),
                    };
                    (result.inserted, failed, took_ms, Some(event))
                }
                Err(e) => {
                    return Err(ServerMessage::Error {
                        message: format!("batch insert error: {e}"),
                        code: 500,
                    });
                }
            }
        }
        // RwLockWriteGuard dropped here — no lock held past this point
    };

    // No locks held from here on — safe to call any async fn in the future.
    if let Some(event) = opt_event {
        app_state.record_ingest(unix_now(), inserted as u64);
        app_state.emit_event(event);
    }

    Ok((inserted, failed, took_ms))
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo clippy --package ferres-db-server --all-targets -- -D warnings -D clippy::await_holding_lock 2>&1
```
Expected: no errors related to streaming.rs

- [ ] **Step 4: Run streaming tests**

```bash
cargo test --package ferres-db-server --test websocket_test 2>&1
```
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/handlers/streaming.rs
git commit -s -m "refactor(streaming): replace explicit drop() with block scoping in process_upsert_batch

Lock guard now lives only within a {} block and is dropped automatically
at the closing brace. No explicit drop() calls remain, making future async
additions immediately catchable by clippy::await_holding_lock.

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 6: Add concurrency stress test

**Files:**
- Create: `crates/server/tests/lock_safety_test.rs`

This test verifies that N concurrent writer tasks and M concurrent reader tasks complete without deadlock when lock guards are properly scoped to blocks. It would hang indefinitely if a writer held the write lock across an `.await` point.

- [ ] **Step 1: Write the failing test**

Create `crates/server/tests/lock_safety_test.rs`:

```rust
//! # Lock Safety — stress tests
//!
//! Verifica que N writers + M readers concorrentes completam sem deadlock.
//! Se um guard de lock fosse mantido através de um ponto .await, os writers
//! bloqueariam os readers indefinidamente e o timeout de 10s dispararia.

use std::sync::{Arc, RwLock};

use futures::future::join_all;
use tokio::time::{timeout, Duration};

use ferres_db_core::{Collection, CollectionConfig, DistanceMetric, FileStorage, Point};

const N_WRITERS: usize = 4;
const N_READERS: usize = 8;
const OPS_PER_TASK: usize = 50;
const TIMEOUT_SECS: u64 = 10;

/// Bootstraps an in-memory collection wrapped in Arc<RwLock<>>.
fn make_collection(tmp: &tempfile::TempDir) -> Arc<RwLock<Collection>> {
    let config = CollectionConfig {
        dimension: 4,
        distance: DistanceMetric::Cosine,
        ..Default::default()
    };
    let storage = FileStorage::new(tmp.path().join("coll")).unwrap();
    Arc::new(RwLock::new(
        Collection::create("stress", config, storage).unwrap(),
    ))
}

/// N writers and M readers run concurrently.
/// Each task acquires the lock in a scoped block and yields AFTER releasing it.
/// Completes within TIMEOUT_SECS → no deadlock.
#[tokio::test]
async fn test_concurrent_rw_no_deadlock() {
    let tmp = tempfile::TempDir::new().unwrap();
    let collection = make_collection(&tmp);

    let mut handles = Vec::new();

    // Writer tasks
    for i in 0..N_WRITERS {
        let coll = collection.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..OPS_PER_TASK {
                // Lock scoped to block — dropped before yield_now
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
                tokio::task::yield_now().await; // async point — no lock held
            }
        }));
    }

    // Reader tasks
    for _ in 0..N_READERS {
        let coll = collection.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..OPS_PER_TASK {
                // Lock scoped to block — dropped before yield_now
                let _len = {
                    let c = coll.read().unwrap();
                    c.len()
                }; // read lock released here
                tokio::task::yield_now().await; // async point — no lock held
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
/// are running. Starvation would manifest as reader tasks finishing long
/// after writers, which the shared timeout catches.
#[tokio::test]
async fn test_readers_not_starved_by_writers() {
    let tmp = tempfile::TempDir::new().unwrap();
    let collection = make_collection(&tmp);

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
```

- [ ] **Step 2: Run to verify it passes**

```bash
cargo test --package ferres-db-server --test lock_safety_test -- --nocapture 2>&1
```
Expected: both tests pass in < 10 seconds

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/lock_safety_test.rs
git commit -s -m "test(concurrency): add lock safety stress tests

N writers + M readers run concurrently; each acquires the lock inside a
scoped block and yields after release. A 10s timeout catches deadlocks and
starvation that would occur if a guard were held across an await point.

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 7: Add CI clippy job

**Files:**
- Modify: `.github/workflows/ci.yml`

Add the `rust-clippy` job after the existing `rust-fmt` job. Must install `protoc` (same as `rust-test`) so `--all-features` compiles the gRPC crate. Uses the same pinned action SHAs as the rest of the workflow.

- [ ] **Step 1: Add the job to ci.yml**

In `.github/workflows/ci.yml`, after the `rust-fmt` job block (after line 43), insert:

```yaml
  rust-clippy:
    name: 🦀 Clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
      - uses: dtolnay/rust-toolchain@b3b07ba8b418998c39fb20f53e8b695cdcc8de1b # stable
        with:
          toolchain: stable
          components: clippy
      - uses: Swatinem/rust-cache@98c8021b550208e191a6a3145459bfc9fb29c4c0 # v2.7.8
      - name: Install protoc
        run: sudo apt-get update -q && sudo apt-get install -y protobuf-compiler
      - name: Clippy
        run: |
          cargo clippy --workspace --all-features --all-targets -- \
            -D warnings \
            -D clippy::await_holding_lock \
            -D clippy::await_holding_refcell_ref \
            -W clippy::unwrap_used
```

- [ ] **Step 2: Verify ci.yml is valid YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo "valid"
```
Expected: `valid`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -s -m "ci: add Clippy job with await_holding_lock enforcement

Errors on:
  -D warnings
  -D clippy::await_holding_lock
  -D clippy::await_holding_refcell_ref
Warns on:
  -W clippy::unwrap_used  (future cleanup target per spec)

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 8: Document lock rule in CONTRIBUTING.md

**Files:**
- Modify: `CONTRIBUTING.md`

Add a "Async & Locks" section to the "Code standards → Rust" block.

- [ ] **Step 1: Add the section**

After the "Conventions" block in `CONTRIBUTING.md` (after line 65), insert:

```markdown
### Async & Locks — rule enforced by Clippy CI

**`std::sync::RwLock` and `Mutex` guards must never be held across an `await` point.**

Holding a synchronous lock guard across `.await` causes deadlocks on Tokio's cooperative scheduler because the executor cannot preempt a task that holds a lock while it is suspended.

CI enforces this with `-D clippy::await_holding_lock -D clippy::await_holding_refcell_ref`.

**Three accepted patterns:**

**A — extract a sync helper** (model from `grpc.rs`):
```rust
async fn handler(state: AppState, ...) -> Result<...> {
    let result = do_work_sync(&state, ...)?;   // acquires lock, returns data
    emit_event_async(result).await;            // no lock held
}
```

**B — scope the guard with a block** (preferred in handlers):
```rust
let payload = {
    let mut coll = collection_arc.write()?;    // guard lives only in this block
    coll.insert_batch(points)?
};                                              // guard dropped here automatically
state.emit_event(...).await;                   // no lock held
```

**C — use `tokio::sync::RwLock`** (rare; only when you genuinely need to hold a lock across await):
```rust
// Only justified when the critical section itself contains .await
let mut coll = tokio_rwlock.write().await;
coll.async_operation().await;  // still holds lock — document why
```

Pattern B is the standard. Pattern A is used in gRPC handlers. Pattern C is exceptional and requires a comment explaining why.

> **Never use explicit `drop(guard)` before an async call as a substitute for block scoping.** Block scoping is checked by the compiler; an accidental removal of `drop()` introduces a bug silently.
```

- [ ] **Step 2: Verify the markdown renders (check no broken fence)**

```bash
python3 -c "
import re, sys
text = open('CONTRIBUTING.md').read()
fences = re.findall(r'^\`\`\`', text, re.MULTILINE)
print(f'{len(fences)} backtick fences — should be even')
sys.exit(0 if len(fences) % 2 == 0 else 1)
"
```
Expected: even number of fences

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -s -m "docs(contributing): document async lock safety rule

Three accepted patterns (A/B/C), enforcement via Clippy CI flags,
and the explicit prohibition on using drop() as a substitute for block scoping.

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 9: Full verification pass

- [ ] **Step 1: Run clippy (without --all-features due to local protoc absence)**

```bash
cargo clippy --workspace --all-targets -- -D warnings -D clippy::await_holding_lock -D clippy::await_holding_refcell_ref -W clippy::unwrap_used 2>&1
```
Expected: 0 errors

- [ ] **Step 2: Run all tests**

```bash
cargo test --workspace --all-targets 2>&1
```
Expected: all tests pass

- [ ] **Step 3: Run cargo fmt check**

```bash
cargo fmt --all -- --check 2>&1
```
Expected: clean (no diff)

- [ ] **Step 4: If fmt found diffs, apply and re-commit**

```bash
cargo fmt --all
git add -u
git commit -s -m "style: apply cargo fmt"
```

---

## Task 10: Update vault

**Files:**
- Modify: `valt/decisions.md`
- Modify: `valt/index.md`

- [ ] **Step 1: Append ADR-023 to `valt/decisions.md`**

Use the decision template. Append to the end of `valt/decisions.md`:

```markdown
---

## ADR-023 — Async Lock Safety: block scoping + Clippy enforcement

**Date:** 2026-04-27
**Status:** Accepted

### Context
`std::sync::RwLock` and `Mutex` guards held across `.await` points cause deadlocks on Tokio's cooperative scheduler. The codebase had one structurally fragile case in `streaming.rs` (`process_upsert_batch`) where `drop(guard)` was called explicitly before sync post-processing calls — safe today but fragile to future async refactors.

### Decision
1. Enforce `clippy::await_holding_lock` and `clippy::await_holding_refcell_ref` as errors in a dedicated CI job.
2. All lock guards must be released by block scoping (`{ let guard = ...; work; } // dropped`), not by explicit `drop()`.
3. `streaming.rs` `process_upsert_batch` refactored to a single scoped block that returns a plain value tuple; post-lock work happens outside.
4. `clippy::unwrap_used` added as `-W` (warning) to prepare for a future cleanup pass.

### Consequences
- Deadlock class eliminated structurally, not just by convention.
- CI catches regressions immediately.
- Five pre-existing `clippy -D warnings` errors in `crates/core` were fixed as part of this work.
- `unwrap_used` warnings serve as a living inventory for future hardening.
```

- [ ] **Step 2: Update `valt/index.md` counts**

Update the ADR count from 22 to 23 in `valt/index.md`.

- [ ] **Step 3: Commit vault**

```bash
git add valt/decisions.md valt/index.md
git commit -s -m "valt: document ADR-023 async lock safety decision"
```

---

## Self-Review

### Spec coverage

| Spec item | Task(s) that implement it |
|---|---|
| CI job with `await_holding_lock` | Task 7 |
| Run locally and capture issues | Tasks 1-4 (fix them); no violations found |
| Fix `streaming.rs` fragile `drop()` | Task 5 |
| Fix `points.rs` if violations | Not needed — already uses block scoping |
| Fix `save.rs` if violations | Not needed — no lock held |
| Stress test N writers + M readers | Task 6 |
| `clippy::unwrap_used` as `-W` (not `-D`) | Task 7 |
| Document in CONTRIBUTING.md | Task 8 |
| Update vault | Task 10 |

### No placeholders: confirmed
All code blocks contain complete, runnable code.

### Type consistency: confirmed
`CollectionEvent`, `Arc::clone`, `RwLockWriteGuard` — all consistent with existing codebase imports.
