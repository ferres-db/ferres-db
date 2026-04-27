# Unwrap → Expect Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every silent `unwrap()` in production code with `expect("reason")` that explains the invariant, so panics include actionable context.

**Architecture:** All changes are purely mechanical substitutions in existing production files. Tests and benchmarks keep their `unwrap()` calls unchanged (per project convention). Lock-poison cases — where the function cannot return `Result` — get an `expect` describing which lock was poisoned. Cases where `expect` is logically impossible (guarded by `if > 0`, `contains_key`, `is_none()` early-return, etc.) explain the guard. CI already has `-W clippy::unwrap_used`; we add `-W clippy::expect_used` as well, and update CONTRIBUTING.md with the error-handling contract.

**Tech Stack:** Rust stable, Clippy, GitHub Actions CI

---

## Inventory: production `unwrap()` locations

| File | Line(s) | Category | Invariant |
|---|---|---|---|
| `crates/core/src/collection.rs` | 197, 234 | guard | `if cache_size > 0` check above |
| `crates/core/src/collection.rs` | 355 | just-inserted | field inserted on line 353 |
| `crates/core/src/collection.rs` | 474 | non-empty iter | `if !validation_errors.is_empty()` |
| `crates/core/src/collection.rs` | 703, 726, 774, 833 | contains_key | `contains_key(f)` validated above |
| `crates/core/src/tiered.rs` | 368 | impossible | `chunks_exact(4)` guarantees 4-byte slice |
| `crates/core/src/tiered.rs` | 646 | impossible | `write!` to `String` is infallible |
| `crates/core/src/tiered.rs` | 1307 | lock poison | `point_tiers.read()` fn returns non-Result |
| `crates/core/src/tiered.rs` | 1459 | lock poison | `access_tracker.lock()` fn returns non-Result |
| `crates/core/src/wal.rs` | 480 | guard | `writer` is `Some` when `needs_fsync` |
| `crates/core/src/lib.rs` | 712 | non-empty iter | `if !validation_errors.is_empty()` |
| `crates/core/src/search.rs` | 1131 | early-return | `if self.params.is_none() { return; }` above |
| `crates/server/src/metrics.rs` | 17–102 (×12) | impossible | hardcoded static metric names |
| `crates/server/src/metrics.rs` | 109 | impossible | `TextEncoder` to `String` is infallible |
| `crates/server/src/handlers/collections.rs` | 31 | impossible | hardcoded regex literal |
| `crates/server/src/handlers/metrics.rs` | 18 | impossible | `Response::builder()` with valid status + body |
| `crates/server/src/query_log_analytics.rs` | 69 | lock poison | cache `RwLock::write()` fn returns non-Result |
| `crates/server/src/query_log_analytics.rs` | 80 | guard | cache just set to `Some` in refresh branch |
| `crates/server/src/state.rs` | 127 | lock poison | `events.read()` fn returns non-Result |
| `crates/server/src/state.rs` | 524 | lock poison | `latencies_ms.read()` fn returns non-Result |

---

## Task 1: Fix `crates/core/src/collection.rs`

**Files:**
- Modify: `crates/core/src/collection.rs:197,234,355,474,703,726,774,833`

### Step 1.1 — Lines 197 and 234: `NonZeroUsize::new`

- [ ] In `Collection::new` (line 197):

```rust
// BEFORE
std::num::NonZeroUsize::new(config.search_cache_size).unwrap(),

// AFTER
std::num::NonZeroUsize::new(config.search_cache_size)
    .expect("search_cache_size > 0 was checked in the enclosing if-guard"),
```

- [ ] In `Collection::with_index` (line 234), same change:

```rust
// BEFORE
std::num::NonZeroUsize::new(config.search_cache_size).unwrap(),

// AFTER
std::num::NonZeroUsize::new(config.search_cache_size)
    .expect("search_cache_size > 0 was checked in the enclosing if-guard"),
```

### Step 1.2 — Line 355: `get_mut` after insert

- [ ] In `get_or_create_vector_index`:

```rust
// BEFORE
Ok(self.vector_indices.get_mut(field).unwrap())

// AFTER
Ok(self.vector_indices
    .get_mut(field)
    .expect("field was just inserted into vector_indices on the line above"))
```

### Step 1.3 — Line 474: `next()` on non-empty Vec

- [ ] In `insert_batch` parallel validation:

```rust
// BEFORE
return Err(validation_errors.into_iter().next().unwrap());

// AFTER
return Err(validation_errors
    .into_iter()
    .next()
    .expect("validation_errors is non-empty — checked by the if-guard above"));
```

### Step 1.4 — Lines 703, 726, 774, 833: `vector_indices.get` after `contains_key`

Each occurrence follows a `contains_key(f)` guard that returns `Err(UnknownVectorField)` if absent.

- [ ] Line 703 (`search`, cache path):

```rust
// BEFORE
Some(f) => self.vector_indices.get(f).unwrap().search(query, k, None)?,

// AFTER
Some(f) => self
    .vector_indices
    .get(f)
    .expect("vector field validated by contains_key above")
    .search(query, k, None)?,
```

- [ ] Lines 722-727 (`search`, non-cache path):

```rust
// BEFORE
Some(f) => self
    .vector_indices
    .get(f)
    .unwrap()
    .search(query, k, predicate),

// AFTER
Some(f) => self
    .vector_indices
    .get(f)
    .expect("vector field validated by contains_key above")
    .search(query, k, predicate),
```

- [ ] Lines 772-775 (`search_with_rerank`):

```rust
// BEFORE
self.vector_indices
    .get(f)
    .unwrap()
    .search(query, k_candidates, predicate)?

// AFTER
self.vector_indices
    .get(f)
    .expect("vector field validated by contains_key above")
    .search(query, k_candidates, predicate)?
```

- [ ] Lines 830-834 (`search_explain`):

```rust
// BEFORE
Some(f) => self
    .vector_indices
    .get(f)
    .unwrap()
    .search_explain(query, k, predicate),

// AFTER
Some(f) => self
    .vector_indices
    .get(f)
    .expect("vector field validated by contains_key above")
    .search_explain(query, k, predicate),
```

### Step 1.5 — Verify and commit

- [ ] Run clippy for the core crate:

```bash
cargo clippy --package ferres-db-core --all-targets -- -W clippy::unwrap_used 2>&1 | grep "collection.rs"
```
Expected: no `unwrap_used` warnings for `collection.rs`

- [ ] Run tests:

```bash
cargo test --package ferres-db-core 2>&1 | tail -5
```
Expected: `test result: ok`

- [ ] Commit:

```bash
git add crates/core/src/collection.rs
git commit -s -m "fix(core): replace unwrap() with expect() in collection.rs

Convert all silent unwrap() calls in production paths to expect() with
messages explaining the invariant: NonZeroUsize guard, just-inserted map
key, non-empty iterator, contains_key validation.

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 2: Fix `crates/core/src/tiered.rs`

**Files:**
- Modify: `crates/core/src/tiered.rs:368,646,1307,1459`

### Step 2.1 — Line 368: `try_into` on `chunks_exact(4)` slice

- [ ] In `WarmStorage::read_vector`:

```rust
// BEFORE
let arr: [u8; 4] = chunk.try_into().unwrap();

// AFTER
let arr: [u8; 4] = chunk
    .try_into()
    .expect("chunks_exact(4) guarantees a 4-byte slice");
```

### Step 2.2 — Line 646: `write!` to `String`

- [ ] In `cold_path_for_id`:

```rust
// BEFORE
write!(&mut safe, "%{byte:02X}").unwrap();

// AFTER
write!(&mut safe, "%{byte:02X}")
    .expect("writing percent-encoding to String is infallible");
```

### Step 2.3 — Line 1307: `point_tiers.read()` lock

`tier_distribution` returns `TierDistribution` (not `Result`), so the lock poison cannot be propagated — expect is the correct choice.

- [ ] In `tier_distribution`:

```rust
// BEFORE
let tiers = self.point_tiers.read().unwrap();

// AFTER
let tiers = self
    .point_tiers
    .read()
    .expect("point_tiers RwLock poisoned — another thread panicked while holding a write guard");
```

### Step 2.4 — Line 1459: `access_tracker.lock()` lock

`from_tiered_collection` returns `Self` (not `Result`).

- [ ] In `TierMetadata::from_tiered_collection`:

```rust
// BEFORE
let tracker = tc.access_tracker.lock().unwrap();

// AFTER
let tracker = tc
    .access_tracker
    .lock()
    .expect("access_tracker Mutex poisoned — another thread panicked while holding the guard");
```

### Step 2.5 — Verify and commit

- [ ] Run clippy:

```bash
cargo clippy --package ferres-db-core --all-targets -- -W clippy::unwrap_used 2>&1 | grep "tiered.rs"
```
Expected: no `unwrap_used` warnings for production lines in `tiered.rs` (test-block warnings are OK)

- [ ] Run tests:

```bash
cargo test --package ferres-db-core 2>&1 | tail -5
```
Expected: `test result: ok`

- [ ] Commit:

```bash
git add crates/core/src/tiered.rs
git commit -s -m "fix(core): replace unwrap() with expect() in tiered.rs

Four production sites:
- chunks_exact(4) try_into invariant
- write! to String is infallible
- point_tiers RwLock poison (fn returns non-Result)
- access_tracker Mutex poison (fn returns non-Result)

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 3: Fix `crates/core/src/wal.rs`, `lib.rs`, `search.rs`

**Files:**
- Modify: `crates/core/src/wal.rs:480`
- Modify: `crates/core/src/lib.rs:712`
- Modify: `crates/core/src/search.rs:1131`

### Step 3.1 — `wal.rs:480`: `writer.as_ref()` when `needs_fsync`

The writer is `Some` because `needs_fsync` is only true after at least one `append_*` call, which initialises the writer. The `as_ref()` guard protects the early-exit path above.

- [ ] In `Wal::append_inner`, fsync block:

```rust
// BEFORE
self.writer
    .as_ref()
    .unwrap()
    .get_ref()
    .sync_data()

// AFTER
self.writer
    .as_ref()
    .expect("WAL writer is Some when needs_fsync=true (append initialises it)")
    .get_ref()
    .sync_data()
```

### Step 3.2 — `lib.rs:712`: `next()` on non-empty Vec (same pattern as collection.rs:474)

- [ ] In `VectorDB::upsert_points` parallel validation block:

```rust
// BEFORE
return Err(validation_errors.into_iter().next().unwrap());

// AFTER
return Err(validation_errors
    .into_iter()
    .next()
    .expect("validation_errors is non-empty — checked by the if-guard above"));
```

### Step 3.3 — `search.rs:1131`: `params.as_ref()` after `is_none()` early-return

Lines 1116-1128 do `if self.params.is_none() { ...; return; }`, so reaching line 1131 guarantees `params` is `Some`.

- [ ] In `QuantizedHnswIndex::add_point`:

```rust
// BEFORE
let params = self.params.as_ref().unwrap();

// AFTER
let params = self
    .params
    .as_ref()
    .expect("params is Some — the is_none() early-return on line 1116 would have exited");
```

### Step 3.4 — Verify and commit

- [ ] Run clippy:

```bash
cargo clippy --package ferres-db-core --all-targets -- -W clippy::unwrap_used 2>&1 | grep -E "wal\.rs|lib\.rs|search\.rs" | grep -v "test"
```
Expected: no production `unwrap_used` warnings

- [ ] Run tests:

```bash
cargo test --package ferres-db-core 2>&1 | tail -5
```
Expected: `test result: ok`

- [ ] Commit:

```bash
git add crates/core/src/wal.rs crates/core/src/lib.rs crates/core/src/search.rs
git commit -s -m "fix(core): replace unwrap() with expect() in wal/lib/search

- wal.rs: writer.as_ref() is Some when needs_fsync is set
- lib.rs: next() on non-empty validation_errors vec
- search.rs: params.as_ref() guarded by is_none() early-return

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 4: Fix `crates/server/src/metrics.rs`

**Files:**
- Modify: `crates/server/src/metrics.rs:17,25,31,38,46,54,61,68,78,87,95,102,109`

All 12 `lazy_static!` metric registrations use hardcoded string literals and cannot fail unless there's a duplicate name or invalid label — both would be programming errors caught immediately at startup.

### Step 4.1 — Replace all metric registration unwraps

- [ ] For each `register_*!(...).unwrap()` in the `lazy_static!` block, replace `.unwrap()` with `.expect("prometheus metric registration failed: duplicate name or invalid label")`. The full file after the change:

```rust
lazy_static! {
    pub static ref HTTP_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "http_requests_total",
        "Total number of HTTP requests",
        &["method", "endpoint", "status"]
    ).expect("prometheus metric 'http_requests_total' registration failed");

    pub static ref HTTP_REQUEST_DURATION_MS: HistogramVec = register_histogram_vec!(
        "http_request_duration_ms",
        "HTTP request latency in milliseconds",
        &["method", "endpoint"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).expect("prometheus metric 'http_request_duration_ms' registration failed");

    pub static ref COLLECTIONS_ACTIVE: Gauge = register_gauge!(
        "collections_active",
        "Number of active collections"
    ).expect("prometheus metric 'collections_active' registration failed");

    pub static ref QUERIES_TOTAL: CounterVec = register_counter_vec!(
        "queries_total",
        "Total number of search queries",
        &["collection"]
    ).expect("prometheus metric 'queries_total' registration failed");

    pub static ref QUERY_DURATION_MS: HistogramVec = register_histogram_vec!(
        "query_duration_ms",
        "Search query latency in milliseconds",
        &["collection"],
        vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0]
    ).expect("prometheus metric 'query_duration_ms' registration failed");

    pub static ref WS_CONNECTIONS_ACTIVE: Gauge = register_gauge!(
        "ws_connections_active",
        "Number of active WebSocket connections"
    ).expect("prometheus metric 'ws_connections_active' registration failed");

    pub static ref WS_MESSAGES_RECEIVED_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_received_total",
        "Total WebSocket messages received",
        &["message_type"]
    ).expect("prometheus metric 'ws_messages_received_total' registration failed");

    pub static ref WS_MESSAGES_SENT_TOTAL: CounterVec = register_counter_vec!(
        "ws_messages_sent_total",
        "Total WebSocket messages sent",
        &["message_type"]
    ).expect("prometheus metric 'ws_messages_sent_total' registration failed");

    pub static ref LLM_PROXY_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_llm_proxy_requests_total",
        "Total LLM proxy requests by provider and status",
        &["provider", "status"]
    ).expect("prometheus metric 'ferresdb_llm_proxy_requests_total' registration failed");

    pub static ref WAL_FSYNC_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_wal_fsync_total",
        "Total number of WAL fsync calls",
        &["collection"]
    ).expect("prometheus metric 'ferresdb_wal_fsync_total' registration failed");

    pub static ref WAL_FSYNC_DURATION: HistogramVec = register_histogram_vec!(
        "ferresdb_wal_fsync_duration_seconds",
        "WAL fsync latency in seconds",
        &["collection"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    ).expect("prometheus metric 'ferresdb_wal_fsync_duration_seconds' registration failed");

    pub static ref WAL_PENDING_FSYNC_OPS: GaugeVec = register_gauge_vec!(
        "ferresdb_wal_pending_fsync_ops",
        "Number of WAL ops since last fsync",
        &["collection"]
    ).expect("prometheus metric 'ferresdb_wal_pending_fsync_ops' registration failed");
}
```

### Step 4.2 — Line 109: `encoder.encode_to_string`

- [ ] In `gather_metrics`:

```rust
// BEFORE
encoder.encode_to_string(&metric_families).unwrap()

// AFTER
encoder
    .encode_to_string(&metric_families)
    .expect("TextEncoder::encode_to_string failed writing to String")
```

### Step 4.3 — Verify and commit

- [ ] Run clippy for server:

```bash
cargo clippy --package ferres-db-server --all-targets --all-features -- -W clippy::unwrap_used 2>&1 | grep "metrics.rs"
```
Expected: no `unwrap_used` warnings for `metrics.rs`

- [ ] Run tests:

```bash
cargo test --package ferres-db-server 2>&1 | tail -5
```
Expected: `test result: ok`

- [ ] Commit:

```bash
git add crates/server/src/metrics.rs
git commit -s -m "fix(server): replace unwrap() with expect() in metrics.rs

All 12 lazy_static prometheus registrations and the TextEncoder call now
carry messages explaining why each is invariant (hardcoded names, infallible
String write).

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 5: Fix server handlers and analytics

**Files:**
- Modify: `crates/server/src/handlers/collections.rs:31`
- Modify: `crates/server/src/handlers/metrics.rs:18`
- Modify: `crates/server/src/query_log_analytics.rs:69,80`
- Modify: `crates/server/src/state.rs:127,524`

### Step 5.1 — `handlers/collections.rs:31`: hardcoded regex

- [ ] In `lazy_static!`:

```rust
// BEFORE
static ref VALID_NAME_REGEX: regex::Regex = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();

// AFTER
static ref VALID_NAME_REGEX: regex::Regex =
    regex::Regex::new(r"^[a-zA-Z0-9_-]+$")
        .expect("VALID_NAME_REGEX pattern is a hardcoded literal and must always compile");
```

### Step 5.2 — `handlers/metrics.rs:18`: response builder

- [ ] In `get_metrics`:

```rust
// BEFORE
.body(Body::from(metrics))
.unwrap()

// AFTER
.body(Body::from(metrics))
.expect("response builder with valid StatusCode and Body::from(String) never fails")
```

### Step 5.3 — `query_log_analytics.rs:69`: `cache.write()` lock

`get_entries` returns `Vec<ParsedQueryEntry>` — no `Result` to propagate.

- [ ] In `get_entries`:

```rust
// BEFORE
let mut guard = self.cache.write().unwrap();

// AFTER
let mut guard = self
    .cache
    .write()
    .expect("analytics cache RwLock poisoned — a thread panicked while holding a write guard");
```

### Step 5.4 — `query_log_analytics.rs:80`: `guard.as_ref()` after just-populated cache

The `refresh` branch (lines 75-78) sets `*guard = Some(...)`. The `else` branch (line 80) is only reached when `refresh == false`, meaning the cache was already `Some`.

- [ ] In `get_entries`:

```rust
// BEFORE
guard.as_ref().unwrap().0.clone()

// AFTER
guard
    .as_ref()
    .expect("cache is Some — refresh=false branch only reached when cache was previously populated")
    .0
    .clone()
```

### Step 5.5 — `state.rs:127`: `events.read()` lock

`events_last_24h` returns `Vec<GlobalQueryEvent>`.

- [ ] In `events_last_24h`:

```rust
// BEFORE
let events = self.events.read().unwrap();

// AFTER
let events = self
    .events
    .read()
    .expect("events RwLock poisoned — a thread panicked while holding a write guard");
```

### Step 5.6 — `state.rs:524`: `latencies_ms.read()` lock

`calculate_percentiles` returns `(f64, f64, f64, f64)`.

- [ ] In `calculate_percentiles`:

```rust
// BEFORE
let latencies = self.latencies_ms.read().unwrap();

// AFTER
let latencies = self
    .latencies_ms
    .read()
    .expect("latencies_ms RwLock poisoned — a thread panicked while holding a write guard");
```

### Step 5.7 — Verify and commit

- [ ] Run clippy:

```bash
cargo clippy --package ferres-db-server --all-targets --all-features -- -W clippy::unwrap_used 2>&1 | grep -E "handlers/|query_log|state\.rs"
```
Expected: no `unwrap_used` warnings on those files

- [ ] Run tests:

```bash
cargo test --package ferres-db-server 2>&1 | tail -5
```
Expected: `test result: ok`

- [ ] Commit:

```bash
git add crates/server/src/handlers/collections.rs crates/server/src/handlers/metrics.rs \
        crates/server/src/query_log_analytics.rs crates/server/src/state.rs
git commit -s -m "fix(server): replace unwrap() with expect() in handlers, analytics, state

- handlers/collections.rs: hardcoded regex literal
- handlers/metrics.rs: valid response builder
- query_log_analytics.rs: RwLock poison + cache-populated guard
- state.rs: RwLock poison on events and latencies_ms

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 6: Add `clippy::expect_used` warning to CI + update CONTRIBUTING.md

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `CONTRIBUTING.md`

**Note:** `-W clippy::unwrap_used` is already present in CI (added in the async-lock-safety PR). We only need to add `expect_used`.

### Step 6.1 — Add `expect_used` warning to CI

- [ ] In `.github/workflows/ci.yml`, in the `rust-clippy` job's `Clippy` step, add the new flag:

```yaml
# BEFORE
        run: |
          cargo clippy --workspace --all-features --all-targets -- \
            -D warnings \
            -D clippy::await_holding_lock \
            -D clippy::await_holding_refcell_ref \
            -W clippy::unwrap_used

# AFTER
        run: |
          cargo clippy --workspace --all-features --all-targets -- \
            -D warnings \
            -D clippy::await_holding_lock \
            -D clippy::await_holding_refcell_ref \
            -W clippy::unwrap_used \
            -W clippy::expect_used
```

### Step 6.2 — Add `## Error handling` section to CONTRIBUTING.md

- [ ] After the "Async & Locks" section in `CONTRIBUTING.md` (before `## Testing`), insert:

```markdown
### Error handling

- Use `?` or return `Result` for recoverable errors.
- Use `.expect("clear reason")` for invariants that **cannot** fail — the message must explain *why* it cannot fail, not just what the value is.
- **Never** use bare `.unwrap()` in production code (`src/`, `lib.rs`). Tests and benchmarks are exempt.
- For lock poison: if the function returns `Result`, convert with `.map_err(|_| MyError::LockPoisoned)?`. If the function cannot return `Result`, use `.expect("which_lock poisoned — describe what that means")`.

Examples:

```rust
// ✅ OK — guard explains invariant
std::num::NonZeroUsize::new(cache_size)
    .expect("cache_size > 0 was checked in the enclosing if-guard")

// ✅ OK — lock poison, function can't return Result
self.events.read()
    .expect("events RwLock poisoned — another thread panicked while holding a write guard")

// ✅ OK — lock poison, function returns Result
conn.lock().map_err(|_| MyError::LockPoisoned)?

// ❌ BAD — no context when this panics in production
self.events.read().unwrap()
```
```

### Step 6.3 — Verify and commit

- [ ] Verify YAML is valid:

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"
```
Expected: `ok`

- [ ] Commit:

```bash
git add .github/workflows/ci.yml CONTRIBUTING.md
git commit -s -m "ci+docs: add expect_used warning and document error-handling contract

CI now warns on both unwrap_used and expect_used. CONTRIBUTING.md gains
a dedicated error-handling section with the rule and examples.

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

---

## Task 7: Final verification

- [ ] Run full workspace clippy:

```bash
cargo clippy --workspace --all-targets -- \
  -D warnings \
  -D clippy::await_holding_lock \
  -D clippy::await_holding_refcell_ref \
  -W clippy::unwrap_used \
  -W clippy::expect_used 2>&1 | grep "^error" | head -20
```
Expected: 0 errors

- [ ] Run all workspace tests:

```bash
cargo test --workspace 2>&1 | tail -10
```
Expected: all pass

- [ ] Run fmt check:

```bash
cargo fmt --all -- --check 2>&1
```
Expected: no diff

---

## Task 8: Update vault and push

**Files:**
- Modify: `valt/decisions.md`
- Modify: `valt/overview.md`
- Modify: `valt/index.md`
- Modify: `valt/conventions.md`

### Step 8.1 — Append ADR to `valt/decisions.md`

- [ ] Append using the decision template:

```markdown
---

## ADR-024 — Unwrap Hardening: replace `.unwrap()` with `.expect("reason")` in production code

**Date:** 2026-04-27
**Status:** Accepted

### Context
Production `unwrap()` calls produce panics with "called Option::unwrap() on a None value" — no context about which invariant was violated. 19 silent `unwrap()` calls existed across core and server crates. CI already warned on `clippy::unwrap_used` but had not yet been actioned.

### Decision
1. Every production `unwrap()` replaced with `expect("reason")` explaining the invariant.
2. Lock-poison sites use `expect("which lock, what that means")` when the function cannot return `Result`; use `.map_err(|_| E::LockPoisoned)?` when it can (already done in api_keys, users, cloud_settings, llm_credentials).
3. Tests and benchmarks keep bare `unwrap()` — they are exempt by convention.
4. CI gains `-W clippy::expect_used` alongside the existing `-W clippy::unwrap_used`.
5. CONTRIBUTING.md documents the rule and lock-poison patterns.

### Consequences
- Panics in production now carry actionable context (which invariant, why it should hold).
- The living `-W clippy::expect_used` inventory makes future `?`-propagation refactors easy to identify.
- No behaviour changes — all modifications are purely diagnostic.
```

### Step 8.2 — Update `valt/conventions.md` error-handling section

- [ ] Update the error handling section to reference the new rule (append after the existing content):

```markdown
### `.unwrap()` vs `.expect()` vs `?`

- `?` for all recoverable errors at system boundaries.
- `.expect("invariant reason")` for truly impossible failures (guarded values, infallible ops).
  Message must explain *why* it cannot fail.
- `.unwrap()` only in `#[cfg(test)]` blocks and benchmarks.
- Lock poison: `.map_err(|_| E::LockPoisoned)?` if caller returns Result; `.expect("lock+reason")` otherwise.
```

### Step 8.3 — Update `valt/overview.md` status block

- [ ] Update the status block to mention the hardening work.

### Step 8.4 — Update `valt/index.md` ADR count

- [ ] Change `24 ADRs` → `25 ADRs` (was 24 including ADR-023 from the lock-safety PR; now 25 with ADR-024).

### Step 8.5 — Commit vault

```bash
git add valt/decisions.md valt/conventions.md valt/overview.md valt/index.md
git commit -s -m "valt: document ADR-024 unwrap hardening and update conventions

Signed-off-by: Rafael Ferres <games.ferres@gmail.com>"
```

### Step 8.6 — Push to fix/006

```bash
git push origin fix/006
```

---

## Self-Review

### Spec coverage

| Spec item | Task |
|---|---|
| List all `unwrap()` in prod code | Inventory table above |
| Classify each unwrap | Inventory: category column |
| `expect()` for known-invariant sites | Tasks 1–5 |
| Lock-poison handling with `expect` (non-Result fns) | Tasks 2, 5 |
| CI `-W clippy::unwrap_used` | Already present — verified |
| CI `-W clippy::expect_used` | Task 6 |
| CONTRIBUTING.md error-handling section | Task 6 |
| Vault ADR | Task 8 |
| Commit + push to fix/006 | Tasks 1-5 (commits), Task 8 (push) |
| Run tests + clippy after each file | Each task has verify+commit step |

### No placeholders: confirmed
All code blocks show exact before/after.

### Type consistency: confirmed
All types (`NonZeroUsize`, `LruCache`, `TierDistribution`, `RwLock`, `TextEncoder`) match existing imports.
