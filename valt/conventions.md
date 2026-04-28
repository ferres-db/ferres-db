---
name: Coding Conventions
description: Naming, error handling, testing, style, and tooling conventions across all layers
type: project
---

# FerresDB — Coding Conventions

## Naming

### Rust (core + server)
✅ Confirmed — consistent across all source files

- **snake_case** for functions, variables, modules, fields
  _(evidência: `upsert_points`, `collection_dir`, `storage_path`, `search_with_filter` — crates/core/src/lib.rs, crates/server/src/*)_
- **PascalCase** for types, structs, enums, traits
  _(evidência: `VectorDB`, `CollectionConfig`, `FerresError`, `ANNIndex`, `DistanceMetric` — crates/core/src/*)_
- **SCREAMING_SNAKE_CASE** for constants
  _(evidência: `RUNTIME_THREAD_STACK_SIZE`, `MAX_BLOCKING_THREADS`, `AUTO_REINDEX_INTERVAL_SECS` — crates/server/src/main.rs)_
- Module files match their public struct name: `collection.rs` → `Collection`, `search.rs` → `HnswIndex`/`ANNIndex`
- Handler files match route concerns: `handlers/points.rs`, `handlers/collections.rs`
- Route files mirror handler files: `routes/points.rs`, `routes/collections.rs`

### TypeScript/React (dashboard)
✅ Confirmed

- **PascalCase** for React components and interfaces/types
  _(evidência: `MainLayout.tsx`, `CollectionDetails.tsx`, `DropdownMenu.tsx`)_
- **camelCase** for hooks, functions, variables
  _(evidência: `useCollections.ts`, `useApiKeys.ts`, `useWebSocket.ts`)_
- Hook files prefixed with `use`: `useCollections`, `usePoints`, `useStats`
- Page components in `src/pages/`, reusable UI in `src/components/ui/`

### Python SDK
✅ Confirmed — snake_case throughout
_(evidência: `vector_db_client/client.py`, `test_client.py`, `test_models.py`)_

---

## Principles

### Fail fast — strict validation at boundaries
✅ Confirmed — all vectors validated at `Point::new` and `Collection::insert`

Vectors are validated for: empty, NaN/inf, dimension mismatch. Returns explicit error before any mutation.
_(evidência: `crates/core/src/error.rs`, `FerresError` variants; ADR-008)_

### DRY — shared workspace dependencies
✅ Confirmed

All crates share dependency versions via `[workspace.dependencies]` in root `Cargo.toml`.
Server reuses core types directly; no duplication between REST and gRPC handlers.

### Separation of concerns — layered architecture
✅ Confirmed

`core` → pure engine logic (no HTTP, no auth, no metrics)
`server` → all HTTP/gRPC/auth/metrics concerns
`sdk-rust` → HTTP client only

---

## Error handling

### Rust
✅ Confirmed — `thiserror`-derived enums, propagated with `?`

- **Core**: `FerresError` enum with specific variants (`DimensionMismatch`, `CollectionNotFound`, `PointNotFound`, `Storage`, etc.)
  _(evidência: `crates/core/src/error.rs`)_
- **Server**: `ApiError` enum that wraps `FerresError` and maps to HTTP status codes
  _(evidência: `crates/server/src/error.rs`)_
- `api_err!` macro for quick ApiError construction in handlers
- **No panics** in library code: all fallible ops return `Result`

### `.unwrap()` vs `.expect()` vs `?` (ADR-025)

- `?` for all recoverable errors at system boundaries.
- `.expect("invariant reason")` for truly impossible failures — message must explain *why* it cannot fail.
- `.unwrap()` only in `#[cfg(test)]` blocks and benchmarks; never in production `src/`.
- Lock poison: `.map_err(|_| E::LockPoisoned)?` if caller returns `Result`; `.expect("lock+reason")` otherwise.

### TypeScript/Python SDKs
⚠️ Inferred — typed error classes exist (`sdk/typescript/src/errors.ts`, `sdk/python/vector_db_client/exceptions.py`)

---

## Testing

### Rust
✅ Confirmed — unit tests inside `#[cfg(test)]` blocks at the bottom of each module

- `tempfile::TempDir` for isolated disk state in tests
- Helper functions like `create_test_db()` and `create_test_collection()` reused across tests
  _(evidência: `crates/core/src/lib.rs:1439-1462`)_
- Integration tests in `crates/server/tests/` (auth, collections, RBAC, WebSocket)
- `quickcheck` / `quickcheck_macros` for property-based testing
- `criterion` for benchmarks

### Python SDK
✅ Confirmed — pytest, asyncio_mode=auto
_(evidência: `sdk/python/pyproject.toml`, `sdk/python/tests/`)_

### TypeScript SDK
⚠️ Inferred — vitest (see `sdk/typescript/vitest.config.ts`)

---

## Persistence patterns
✅ Confirmed

- **Atomic writes**: temp-file → rename (no partial writes on crash). ADR-009.
- **WAL**: every mutation appended to WAL before in-memory update. Snapshot triggered at threshold (1000 ops by default).
- **Snapshot**: full `points.jsonl` (or `points.bin` with `binary_snapshot=true`) + WAL truncation.
- **MD5 checksum**: stored alongside `points.jsonl` for corruption detection on load.

---

## Code comments language
⚠️ Mixed — doc comments in the core crate are in **Portuguese**; server handlers are mixed; docs/ files are in **English**.
_(evidência: `crates/core/src/lib.rs` (Portuguese), `docs/api.md` (English), README (English))_

---

## Package managers
| Layer | Tool |
|---|---|
| Rust | `cargo` (`Cargo.lock` at workspace root) |
| Dashboard | `npm` (`package-lock.json` in `dashboard/`) |
| TypeScript SDK | `npm` (`package-lock.json` in `sdk/typescript/`) |
| Python SDK | `pip` / `setuptools` (`pyproject.toml` + `setup.py`) |

---

## Build & run
```bash
make build          # cargo build --release --bin ferres-db-server
make run            # cargo run --bin ferres-db-server (dev)
make test           # cargo test --workspace
make docker-build   # docker build -t ferres-db-server:latest .
make docker-run     # docker-compose up -d
```

Optional feature flags: `--features otel`, `--features grpc`, `--features mcp`, `--features rerank`, `--features raft`

---

## Relacionado
- [[overview]] — stack completa e estrutura do projeto
- [[decisions]] — o "por quê" por trás de cada convenção
- [[rust-error-result-pattern]] — snippet: padrão concreto de FerresError + ApiError
- [[rust-test-helper-pattern]] — snippet: padrão concreto de testes Rust
