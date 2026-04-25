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
