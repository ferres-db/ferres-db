# Contribution Guide

Thank you for considering contributing to FerresDB Core. This guide covers the development environment, testing, code standards and PR process.

## Prerequisites

- **Rust**: 1.70+ (`rustup` recommended)
- **Python**: 3.10+ (for scripts, examples and E2E tests)
- **Make** (optional): for `Makefile` targets

## Environment setup

### 1. Clone and build

```bash
git clone <repo-url>
cd ferres-db-core
cargo build
```

### 2. Run tests

```bash
# Unit and integration tests (workspace)
cargo test --workspace

# Or via Makefile
make test
```

### 3. Local server

```bash
cargo run --bin ferres-db-server
# or: make run
```

The server starts at `http://localhost:8080`. API documentation: [docs/api.md](docs/api.md).

## Project structure

| Folder / Crate    | Description                                              |
| ----------------- | -------------------------------------------------------- |
| `crates/core`     | Vector engine: VectorDB, Collection, HNSW, Storage, WAL  |
| `crates/server`   | HTTP server (Axum), handlers, metrics                    |
| `crates/sdk-rust` | Rust client (HTTP, hybrid search)                        |
| `docs/`           | Documentation (API, architecture, ADRs)                  |
| `examples/`       | Ingestion, simple_rag (Python)                           |
| `tests/`          | E2E tests and fixtures                                   |

Architecture and decisions: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/architecture.md](docs/architecture.md), [docs/ADR/](docs/ADR/).

## Code standards

### Rust

- **Formatting**: run `cargo fmt` before committing.
- **Lint**: `cargo clippy --workspace` with no warnings.
- **Docs**: document public APIs with `///`; add doc test examples where they make sense.

### Conventions

- Error handling with `thiserror` and specific types (`FerresError`).
- Logging with `tracing` (avoid `println!` in production code).
- Unit tests in the same file (`#[cfg(test)] mod tests`) or in `tests/` for integration tests.

### Async & Locks — rule enforced by Clippy CI

**`std::sync::RwLock` and `Mutex` guards must never be held across an `await` point.**

Holding a synchronous lock guard across `.await` causes deadlocks on Tokio's cooperative scheduler because the executor cannot preempt a task that holds a lock while it is suspended.

CI enforces this with `-D clippy::await_holding_lock` and `-D clippy::await_holding_refcell_ref`.

**Three accepted patterns:**

**A — extract a sync helper** (model from `crates/server/src/grpc.rs`):

```rust
async fn handler(state: AppState, ...) -> Result<...> {
    let result = do_work_sync(&state, ...)?;   // acquires lock, returns data
    emit_event_async(result).await;            // no lock held
}
```

**B — scope the guard with a block** (standard for handlers):

```rust
let payload = {
    let mut coll = collection_arc.write()?;    // guard lives only in this block
    coll.insert_batch(points)?
};                                              // guard dropped here automatically
state.emit_event(...).await;                   // no lock held
```

**C — use `tokio::sync::RwLock`** (rare; only when the critical section itself contains `.await`):

```rust
let mut coll = tokio_rwlock.write().await;
coll.async_operation().await;  // lock held intentionally — document why
```

Pattern B is the project standard. Pattern A is used in gRPC handlers. Pattern C is exceptional and requires a comment.

> **Never use explicit `drop(guard)` before an async call as a substitute for block scoping.** Block scoping is compiler-enforced; a missing `drop()` call introduces a bug silently.

### Error handling

- Use `?` or return `Result` for recoverable errors.
- Use `.expect("clear reason")` for invariants that **cannot** fail — the message must explain *why* it cannot fail, not just what the value is.
- **Never** use bare `.unwrap()` in production code (`src/`, `lib.rs`). Tests and benchmarks are exempt.
- For lock poison: if the function returns `Result`, convert with `.map_err(|_| MyError::LockPoisoned)?`. If the function cannot return `Result`, use `.expect("which_lock poisoned — describe what that means")`.

Examples:

```rust
// ✅ OK — message explains the invariant
std::num::NonZeroUsize::new(cache_size)
    .expect("cache_size > 0 was checked in the enclosing if-guard")

// ✅ OK — lock poison, function can't return Result
self.events.read()
    .expect("events RwLock poisoned — a thread panicked while holding a write guard")

// ✅ OK — lock poison, function returns Result
conn.lock().map_err(|_| MyError::LockPoisoned)?

// ❌ BAD — no context when this panics in production
self.events.read().unwrap()
```

## Testing

### Unit and integration (Rust)

```bash
cargo test --workspace
```

Includes property tests in `crates/server/tests/`. Make sure all tests pass before opening a PR.

### E2E (Python)

```bash
cd tests/e2e
pip install -r requirements.txt
pytest
```

Recommended to run with the server already running (or via a script that starts/stops the server, if available).

### Benchmarks

```bash
# Generate test corpus (if needed)
python tests/fixtures/generate_corpus.py

cd crates/core
cargo bench
```

## SQLite Schema Migrations

Schema changes for SQLite stores (api_keys, users, cloud_settings, llm_credentials) are managed through versioned migration slices in `crates/server/src/db/migrations.rs`.

**To add a column or table:**

1. Open `crates/server/src/db/migrations.rs`.
2. Find the relevant `MIGRATIONS_*` slice for your store (e.g., `MIGRATIONS_USERS`).
3. Append a new entry with `version = last_version + 1`:

```rust
Migration {
    version: 4,
    name: "users_add_last_login",
    up: "ALTER TABLE users ADD COLUMN last_login INTEGER DEFAULT NULL",
},
```

4. **Never edit or remove an existing migration** that may have already been applied in production. Only append new ones.
5. Add a test for the new migration if it changes table structure.

The `run_migrations` function in `db::migrations` handles:
- Applying only pending migrations (skips already-applied ones via `schema_migrations` table)
- Downgrade detection (returns an error if the DB schema version is ahead of the known migrations)
- Baseline detection (existing databases without `schema_migrations` are baselied automatically on first run)

## Contribution flow

1. **Issue** (recommended): Open an issue describing the change or fix.
2. **Branch**: Create a branch from `main` (e.g. `feature/name` or `fix/description`).
3. **Changes**: Implement with tests and documentation where applicable.
4. **Local checks**:
   - `cargo fmt`
   - `cargo clippy --workspace`
   - `cargo test --workspace`
5. **Commit**: Write clear messages; prefer imperative ("Add X" instead of "Added X").
6. **Pull Request**: Describe what was done and reference the issue, if any. A maintainer will review it.

## Documentation

- **REST API**: [docs/api.md](docs/api.md)
- **Architecture**: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/architecture.md](docs/architecture.md)
- **Decisions (ADRs)**: [docs/ADR/](docs/ADR/)
- **SDK and clients**: [docs/sdk.md](docs/sdk.md)

When adding new behavior or changing contracts, update the corresponding documentation and, if it is a design decision, consider a new ADR in `docs/ADR/`.

## Developer Certificate of Origin (DCO)

All contributions must be signed off with `Signed-off-by`, indicating that you agree to the [Developer Certificate of Origin](DCO.txt).

To sign off automatically, use the `-s` flag when committing:

```bash
git commit -s -m "Add new feature"
```

This appends a line like:

```
Signed-off-by: Your Name your-email@domain.com
```

Set your name and email in git once:

```bash
git config user.name "Your Name"
git config user.email "your-email@domain.com"
```

PRs missing `Signed-off-by` in any commit will not be accepted.

## Questions

If you have questions about architecture or where to implement something, consult the documentation in `docs/` or open an issue.

Thank you for contributing.
