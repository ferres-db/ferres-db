# End-to-end tests

E2E tests for FerresDB: server lifecycle, ingestion, RAG pipeline, and persistence.

## Prerequisites

- **Rust**: server binary is built with `cargo build --release --bin ferres-db-server` (done automatically if missing).
- **Python 3**: install dependencies from this directory.

## Install

From the repo root or from `tests/e2e`:

```bash
pip install -r tests/e2e/requirements.txt
```

For the full RAG pipeline and concurrent ingestion tests (local embeddings):

```bash
pip install -r examples/ingestion/requirements.txt
pip install -r examples/simple_rag/requirements.txt
```

## Run

From the repo root:

```bash
# All e2e tests (sequential)
pytest tests/e2e/ -v

# Parallel with pytest-xdist (recommended)
pytest tests/e2e/ -n auto -v

# Specific test file
pytest tests/e2e/test_server_restart.py -v
```

## Tests

| Test | Description |
|------|-------------|
| `test_full_rag_pipeline` | Starts server, ingests 10 docs, runs 5 questions through RAG; asserts keywords in answers and latency < 3s. **Requires**: `sentence_transformers`, `OPENAI_API_KEY` (skipped if unset). |
| `test_concurrent_ingestion` | 3 processes ingest simultaneously into the same collection; asserts no race conditions and search works. **Requires**: `sentence_transformers`. |
| `test_server_restart` | Inserts 1000 points, kills server, restarts with same storage; asserts 1000 points and search. No extra deps. |

## Environment

- **OPENAI_API_KEY**: optional; if set, `test_full_rag_pipeline` runs (otherwise skipped).
- **PORT / STORAGE_PATH**: overridden per test via fixtures; no need to set for normal runs.
- With **pytest-xdist** (`-n auto` or `-n 3`), each worker gets a distinct port and storage dir to avoid conflicts.

## Skip conditions

- `test_full_rag_pipeline`: skipped if `sentence_transformers` is not installed or `OPENAI_API_KEY` is not set.
- `test_concurrent_ingestion`: skipped if `sentence_transformers` is not installed.
