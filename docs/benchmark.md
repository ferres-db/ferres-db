# FerresDB — Benchmark and Stress Test

Official **benchmark** and **stress test** tool for FerresDB. Allows evaluating ingestion throughput, QPS and latency (P95/P99) for vector searches, and robustness under mixed load (chaos).

## Prerequisites

- FerresDB server running (e.g. `make run` or `cargo run -p ferres-db-server`).
- Default URL: `http://localhost:8080` (overridable with `--url`).
- If the server requires authentication: FerresDB **API key** (see [docs/api.md](api.md) — header `Authorization: Bearer <api-key>`). Pass `--api-key <key>` or set `FERRESDB_API_KEY`.
- For ingest/search with **real embeddings** (OpenAI): OpenAI API key via `--openai-api-key <key>` or `OPENAI_API_KEY` variable. The tool creates the collection, generates texts, calls the OpenAI embeddings API and inserts/searches with those vectors.

## Installation

The tool is a workspace binary:

```bash
cargo build --release -p ferres-bench
```

The executable is at `target/release/ferres-bench` (or `ferres-bench.exe` on Windows).

## Quick usage (Makefile)

```bash
# Build and run a standard test (ingest 10k vectors + search 15s)
make bench

# Ingest only (10k vectors, dim 768, concurrency 20)
make bench-ingest

# Search only (15s, concurrency 50)
make bench-search

# Chaos mode (30s, writers + readers + collection creation)
make bench-chaos
```

## Operation modes

### 1. Ingest (Write Stress)

Creates the collection (if it doesn't exist), generates vectors and fires concurrent inserts. With `--openai-api-key`, uses **OpenAI embeddings**: generates texts ("Document N..."), calls the embeddings API and inserts the vectors (dimension comes from the model, e.g. 1536 for `text-embedding-3-small`). Without OpenAI, uses random vectors and `--dim` (or default 1536).

```bash
# With FerresDB API key and OpenAI embeddings (recommended for realistic scenario)
ferres-bench ingest --api-key <ferres-api-key> --openai-api-key <openai-key> --vectors 10000 --concurrency 20

# Random vectors only (without OpenAI)
ferres-bench ingest --vectors 100000 --dim 768 --concurrency 50
```

| Argument            | Default                | Description                                                          |
| ------------------- | ---------------------- | -------------------------------------------------------------------- |
| `--api-key`         | —                      | FerresDB API key (env: `FERRESDB_API_KEY`). See docs/api.md.         |
| `--vectors`         | 10000                  | Total number of vectors to insert                                    |
| `--dim`             | —                      | Dimension (only without OpenAI). With OpenAI, dimension comes from model. |
| `--concurrency`     | 50                     | Number of concurrent tasks                                           |
| `--collection`      | bench                  | Collection name (automatically created if it doesn't exist)          |
| `--openai-api-key`  | —                      | OpenAI key for embeddings (env: `OPENAI_API_KEY`)                    |
| `--embedding-model` | text-embedding-3-small | OpenAI model (dimension 1536 or 3072 for large)                      |
| `--url`             | localhost:8080         | Server base URL                                                      |

**Displayed metrics:** throughput (vectors/sec), total time. Markdown report at the end.

### 2. Search (Read Stress)

Continuously performs vector searches for a set duration. The mode uses a **two-phase architecture** so that the reported latency is **exclusively from FerresDB** (without embedding API call overhead inside the measurement loop):

- **Phase 1 — Preparation:** Pre-computes a pool of query vectors. With `--openai-api-key`, generates N texts ("Query about topic 0..N...") and calls the OpenAI embeddings API in batch; without OpenAI, generates N random vectors. No measurement is done in this phase.
- **Phase 2 — Benchmark:** Workers consume vectors from the pool (round-robin) and fire only `POST .../search` against FerresDB. The timer measures only the time of each HTTP request (round-trip to the server).

Thus, QPS and latency (min, avg, P50/P90/P95/P99, max) reflect only FerresDB's capacity, not the OpenAI API's. For representative benchmarks of pure FerresDB, random vectors (`--dim 1536`) can be used. The OpenAI mode guarantees realistic vectors, but embedding generation is pre-computed in Phase 1 and does **not** affect the measurement.

```bash
# With FerresDB API key + OpenAI (pre-computed embedding pool)
ferres-bench search --api-key <ferres-api-key> --openai-api-key <openai-key> --duration 60s --concurrency 100

# Random vectors (without OpenAI)
ferres-bench search --duration 60s --concurrency 100 --dim 768
```

| Argument            | Default                | Description                                                          |
| ------------------- | ---------------------- | -------------------------------------------------------------------- |
| `--api-key`         | —                      | FerresDB API key (env: `FERRESDB_API_KEY`)                           |
| `--duration`        | 60s                    | Test duration (e.g. `30s`, `2m`)                                     |
| `--concurrency`     | 100                    | Number of concurrent workers                                         |
| `--collection`      | bench                  | Collection name (must exist and have data)                           |
| `--dim`             | —                      | Search vector dimension (with OpenAI, comes from model)              |
| `--openai-api-key`  | —                      | OpenAI key for query embeddings (env: `OPENAI_API_KEY`)              |
| `--embedding-model` | text-embedding-3-small | OpenAI model                                                         |
| `--num-queries`     | 200                    | Query vector pool size (pre-computed in Phase 1)                     |
| `--limit`           | 10                     | Number of results per search                                         |
| `--warmup`          | 10                     | Warmup requests (not counted) before Phase 2                         |
| `--url`             | localhost:8080         | Server base URL                                                      |

**Displayed metrics:** Query Pool, Total Requests, Successful (count and %), Duration, QPS, Latency Min/Avg/P50/P90/P95/P99/Max (HDR Histogram). Markdown report at the end.

### 3. Chaos (Mixed)

Runs writes, reads and collection creation **simultaneously** to test RwLock and WAL robustness.

```bash
ferres-bench chaos --duration 30s --writers 20 --readers 50
```

| Argument     | Default        | Description                  |
| ------------ | -------------- | ---------------------------- |
| `--duration` | 30s            | Test duration                |
| `--writers`  | 20             | Number of write workers      |
| `--readers`  | 50             | Number of read workers       |
| `--dim`      | 768            | Vector dimension             |
| `--url`      | localhost:8080 | Server base URL              |

**Displayed metrics:** total points written, total searches, collections created. Markdown report at the end.

## Report example (Markdown)

At the end of execution, the tool prints a Markdown block, for example:

```markdown
---
## FerresDB Benchmark Report — Search

| Metric | Value |
|--------|-------|
| Mode | Search (Read Stress) |
| Query Pool | 200 vectors (OpenAI text-embedding-3-small) |
| Total Requests | 95,230 |
| Successful | 95,230 (100.0%) |
| Duration | 15.02s |
| QPS | 6,342 |
| Latency Min | 2.10ms |
| Latency Avg | 12.50ms |
| Latency P50 | 11.20ms |
| Latency P90 | 16.80ms |
| Latency P95 | 18.20ms |
| Latency P99 | 24.10ms |
| Latency Max | 45.00ms |

---
```

Can be copied directly into issues, PRs or posts (GitHub/LinkedIn).

## Technical goal: Backpressure and Connection Pooling

The tool was designed to help validate whether the system can handle **many simultaneous connections** (e.g. 10k) without going down. Usage suggestions:

1. **Progressively increase concurrency** in `ingest` and `search` (e.g. 50 → 200 → 1000).
2. **Measure P99** with `search`: the HDR Histogram provides precise percentiles for evaluating tail latency.
3. **Chaos mode** to stress the RwLock and WAL with a mix of writes, reads and collection creation.

Make sure the server is compiled in **release** mode and, if necessary, adjust OS resource limits (file descriptors, connections) for tests with thousands of connections.

## Environment variables

- **FERRESDB_URL** — Overrides the `--url` default (e.g. `export FERRESDB_URL=http://prod:8080`).
- **FERRESDB_API_KEY** — FerresDB API key when `--api-key` is not passed (as per [docs/api.md](api.md)).
- **OPENAI_API_KEY** — OpenAI API key for `ingest` and `search` when `--openai-api-key` is not passed.

## Crate dependencies

The `ferres-bench` crate uses: `tokio`, `clap`, `rand`, `indicatif`, `hdrhistogram`, `humantime` and `ferres-db-sdk` (REST). No extra configuration is needed beyond the workspace.
