# Changelog

Notable changes to the project, grouped by week. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Released] - 31/06/2026

### Added

- **Optimization: QJL (Quantized Johnson-Lindenstrauss) residual correction for SQ8** — New optional post-quantization correction step in `QuantizedHnswIndex`. After SQ8 quantizes each vector, the residual error (`original - dequantize(quantized)`) is projected by a random Johnson-Lindenstrauss matrix R ∈ {+1,-1}^{m×d} and compressed to 1 bit per projected dimension (packed in u64, `ceil(m/64)` words). During search, after asymmetric SQ8 re-ranking, the unbiased estimator `score_final = score_sq8 − (2/m)·Σᵢ(q_projected_i·sign_i)` is applied, reducing quantization bias and improving recall@10. Enabled via `ScalarQuantizationConfig { enable_qjl: true, qjl_m: 64, qjl_seed: 42 }` — disabled by default, with no impact on existing collections (serde-compatible: fields use `#[serde(default)]`). Matrix R is generated deterministically from `seed` (using `StdRng`) and is not serialized — only `(m, dim, seed)` are persisted and R is regenerated on load. `QjlParams` exposed in the public crate via `pub use quantization::QjlParams`. Dashboard: "Enable QJL residual correction" toggle + dimension slider m (32–128) in SQ8 collection creation; "QJL" badge in collection list. Criterion benchmark: `benchmark_qjl_latency` compares search latency and recall@10 with and without QJL (dim 128 and 384, 1,000 vectors).

- **PolarQuant quantization** (`QuantizationConfig::Polar`): new quantization variant based on recursive polar coordinates. Converts Cartesian vectors into `(final_radius: f32, angles: Vec<u8>)` by recursively grouping pairs `(x, y)` → `(r, θ)` until a single scalar radius remains. Requires no per-block calibration — angles have fixed boundaries in `[0, 2π]`, eliminating the `min`/`max`/`scale` parameter overhead per dimension present in SQ8. Configurable via `PolarQuantConfig { bits_per_angle: u8 }` (default: 8 bits = 256 levels). Exposes `polar_encode`, `polar_decode` and `polar_distance_asymmetric` (query remains as `f32`, candidate decoded on-the-fly). Added `PolarQuantHnswIndex` implementing the `ANNIndex` trait, integrated in the `create_ann_index` factory. Recall ≥ 0.90 in unit tests for dim 128 (1,000 random vectors).
- **Benchmark `quantization_comparison`** (criterion): compares build time, search latency, recall@10 and memory footprint between SQ8 and PolarQuant for dim 128 and 384.

- **Feature: Graph persistence and traversal — graphs over points.** — Core: optional field `relations: Option<Vec<String>>` in `Point` (IDs of related points); method `Collection::add_relation(from_id, to_id)` for undirected graph; persistence in storage (PointBin and JSONL include `relations`) and WAL with new operation `Operation::Link { from, to }` and `Wal::append_link`; Link replay on recovery and PITR. API: `POST /api/v1/collections/{name}/points/link` (body: `{ "from", "to" }`) to create a relation between two points; `GET /api/v1/collections/{name}/points/{id}` and listing include `relations` in the response. Module `crates/core/src/graph.rs`: function `traverse_bfs(get_point, start_id, max_depth)` (BFS with `VecDeque`); `search.rs`: public function `distance_between(a, b, metric)`; `Collection::search_connected(query_vector, center_point_id, hops, k)` — restricts candidates to the subgraph via BFS and returns top K by vector similarity. Endpoint `GET /api/v1/collections/{name}/graph/subgraph`: parameters `center_id` and `depth` for BFS subgraph; `seed` (1-hop) and `limit` (full graph); response in format `{ "nodes": [...], "edges": [...] }`. Dashboard: `react-force-graph-2d` dependency; new **Graph Explorer** page (`/collections/:name/graph`) with force-directed visualization, node click to expand (subgraph API call), colors by metadata (e.g. category) and sidebar with selected node JSON; **CollectionDetails** page with "View Points" and "View Graph" buttons; "View Graph" button also on the points page. Documentation in `docs/api.md` (link, subgraph with center_id/depth/edges, relations field in GET point).

- **Architecture: Added foundation for Raft-based distributed consensus.** — Server: optional crate `openraft` (feature `raft`); types and cluster status API for when a Raft node is run. Consensus: logic prepared so WAL can be replicated to a majority of nodes before confirm (propose via `replicate_then_confirm` once a Raft node is initialized). Dashboard: new **Cluster** page showing active nodes, Leader, and replication status (`GET /api/v1/cluster`). Build with `--features raft` to enable Raft types and future multi-node setup.

- **Feature: Retention Policy Manager — automatic cleanup of old data.** — Core: new `retention_days` configuration in `CollectionConfig` (optional; default `None` = keep indefinitely). Background worker running every 1 hour compacts the WAL (`wal.log`) per collection, removing entries older than the configured period (`compact_wal_entries_older_than` in core). Dashboard: on the **Settings** page, new **Data retention** section to configure retention (days) per collection; changes are persisted via `PATCH /api/v1/collections/{name}` (body: `{ "retention_days": number | null }`). Collection creation accepts optional `retention_days`; `GET /api/v1/collections` and `GET /api/v1/collections/{name}` include `retention_days`. Documentation in `docs/api.md`.

- **Feature: Integrated native Cross-Encoder re-ranking via ONNX Runtime.** — Core: optional support for the `ort` crate (feature `rerank`) to load Cross-Encoder models (e.g. BGE-Reranker). New method `search_with_rerank`: retrieves `limit * 5` candidates via HNSW, re-scores with the Cross-Encoder and returns the top `limit` reordered. API: `rerank: bool` parameter in the body of `POST /api/v1/collections/{name}/search`; response includes `rerank_ms` when applicable. Dashboard (Analytics): "Re-ranking Overhead (ms)" metric. Documentation in `docs/api.md`.

- **Security: Enhanced RBAC with namespace-level access control.** — API keys can be restricted to one or more namespaces (multitenancy). New `NamespaceAllowance` in the permissions model; API key store supports `allowed_namespaces` (create and update via `PUT /api/v1/keys/:id`). Middleware validates namespace from query param `namespace` or header `X-Namespace`; handlers validate namespace from request body (search, upsert, delete points). Dashboard: "Users/API Keys" allows assigning namespaces when creating a key and editing namespaces per key. Documented in `docs/api.md`.

- **Optimization: Dynamic HNSW auto-tuning based on real-time performance metrics.** — The HNSW index now dynamically adjusts `ef_search` (FerresEngine): if P95 latency is low and recall is the priority, the value is increased; if latency is high (proxy for CPU under stress), it is reduced. The auto-tune logic is in `collection.rs` (`apply_hnsw_auto_tune`); the server runs a cycle every 60s using P95 from `query_stats` per collection. New fields in `GET /api/v1/stats/global`: `hnsw_auto_tune_enabled`, `index_optimization_label` ("Optimized by FerresEngine"); in `GET /api/v1/collections/{name}/stats`: `ef_search_current`, `hnsw_auto_tune_enabled`. Dashboard (Overview): badge and "Index" card with "Optimized by FerresEngine". Documentation in `docs/api.md`.

- **Feature: Added Point-in-Time Recovery (PITR) support using timestamped WAL.** — Each WAL entry already includes a Unix timestamp; when saving a snapshot, the server persists `last_snapshot_timestamp` in the collection directory. New endpoint `POST /api/v1/admin/restore` (Admin) accepts `{ "timestamp": <unix_sec>, "collection": "<name>?" }` and restores one or all collections to the state at that moment (loads the last snapshot and replays the WAL only up to the timestamp). `GET /api/v1/admin/restore/points` lists restore points (snapshot + WAL timestamps) per collection. Dashboard: new **Snapshots & Recovery** tab to view restore points and trigger PITR. Documentation in `docs/api.md` (disaster recovery guide).

- **Security: Added secure S3 configuration management and expanded audit logging for admin actions.** — Dashboard Settings: section to configure S3 backup (Bucket, Region, Endpoint); credential fields (Secret Key) are hidden by default (password inputs). New endpoint `POST /api/v1/admin/settings/test-s3` validates S3 connection before saving. All "Backup to S3" and "Reindex" operations triggered via Dashboard (or API) are recorded in the audit log.

- **Optimizations: Added automatic cache warmup on startup.** — On startup, the server reads the last 50 queries from the `query_logger` (queries.log), re-executes them in the background to load HNSW indices into RAM (Hot Tier) and populate the `search_cache`. Tracing logs indicate warmup progress (`warmup: starting cache warmup`, `warmup: ran query`, `warmup: cache warmup completed`). The query log now stores the full vector (optional) to enable replay.

- **Physical storage isolation for improved multitenancy security** — With the `namespace_physical_isolation` option (core: `StorageOptions`, server: `namespace_physical_isolation` in config.toml or `FERRESDB_NAMESPACE_PHYSICAL_ISOLATION`), points of each namespace are written to `data/collections/<name>/namespaces/<namespace>/points.bin` (and optionally `index.bin`). The VectorDB loads indices independently per namespace when those directories exist, enabling per-namespace snapshots and physical data cleanup for one tenant without affecting others. Documentation in `docs/api.md`.

- **S3 Backup** — AWS S3 integration for backups: new endpoint `POST /api/v1/admin/backup` (Admin only) generates a binary snapshot (tar.gz) of the storage directory and uploads it to a configurable S3 bucket. Configuration via `config.toml` or environment variables: **Region** (`s3_region` / `FERRESDB_S3_REGION` or `AWS_REGION`), **Bucket** (`s3_bucket` / `FERRESDB_S3_BUCKET`), **Credentials** (`s3_access_key_id` / `s3_secret_access_key` or `FERRESDB_S3_ACCESS_KEY_ID` / `FERRESDB_S3_SECRET_ACCESS_KEY`, or `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`). Server dependencies: `aws-sdk-s3`, `aws-config`, `tar`, `flate2`. Dashboard: new **Settings** page with "Export to Cloud" button (visible to Admin only). Documentation in `docs/api.md`.

- **Replication (Experimental)** — Foundation for Read Replicas: in core, `Wal::stream_from(collection_dir, position)` for incremental WAL reading; server with `--replica-of <ADDR>` or `FERRESDB_REPLICA_OF` starts as a replica; write endpoints (POST/PUT/DELETE on collections, points, save, reindex, etc.) return **405 Method Not Allowed** on replicas; worker (feature `grpc`) consumes WAL from the leader via gRPC `StreamWal` and applies it to the local VectorDB; dashboard shows "Role: Leader" or "Role: Replica" in Overview; `GET /api/v1/stats/global` includes `role` field. Documentation in `docs/api.md` (Replication section).

- **Dashboard: Visual Explainer in Query Tester.** — In the "Query Tester" tab, the "Explain" button now displays a **Visual Explainer** with the search pipeline: HNSW layers traversed, distance comparisons, points filtered by Native Pre-filtering and results. Total request time (embedding + explain API) is shown. Data comes from the `POST /api/v1/collections/{name}/search/explain` endpoint (`explain_meta`, `candidates_scanned`, `candidates_after_filter`).

- **Dashboard: PITR UI with datetime picker and confirmation.** — On the "Snapshots & Recovery" page: date/time selector (datetime-local) to choose the restore point; "Point-in-Time Restore" button sends the timestamp to `POST /api/v1/admin/restore`. Safety confirmation modal before executing the restore, alerting that the operation resets the database state.

- **Dashboard: Cluster page active in the menu.** — The "Cluster" page remains available in the side menu and displays active nodes returned by `GET /api/v1/cluster`, indicating Leader and Followers (`role` field per node and `leader_id` in the status).

### Documentation

- **api.md:** Final response schema for `POST /api/v1/collections/{name}/search/explain` already documented; added **GET /api/v1/cluster** section with response schema (raft_enabled, leader_id, nodes with id, addr, role, replication_lag). Graph layer documentation: **GET /api/v1/collections/{name}/points/{id}** includes `relations` field; **POST /api/v1/collections/{name}/points/link** (body `from`, `to`); **GET /api/v1/collections/{name}/graph/subgraph** with query params `center_id`, `depth`, `seed`, `limit` and response `{ "nodes", "edges" }`; permissions tables and REST mapping updated with the link endpoint.

## [Released] - 09/02/2026

First stable release of FerresDB, with final performance, analytics and documentation polish.

### Added

- **Performance: SIMD (AVX2/SSE4.1)** — Distance kernels in `crates/core/src/search.rs` using the `pulp` crate: `euclidean_distance` and `dot_product` with runtime dispatch (AVX2 8× f32, SSE4.1 4× f32) and scalar fallback. Asymmetric SQ8 distance (f32×u8) in `quantization.rs` processes multiple bytes in parallel. Server logs on startup via `tracing`: "SIMD acceleration: active" or "scalar fallback". Dashboard (Overview): "SIMD: Active" badge (green) or "SIMD: Scalar" (yellow) from `GET /api/v1/stats/global` (`simd_enabled`).

- **Analytics: correction and visibility** — The `GET /api/v1/stats/analytics` endpoint now populates `time_series_10m` with real data: fresh reading of `queries.log` (without relying on the 1h cache) for `throughput_per_minute`, `recent_latencies` and `p95_latency_ms`. Query logger flushes after each write so analytics reads data immediately. Dashboard: ingestion chart (points/min), latency area chart and P95 histogram (distribution by ms range); "Cache Hit Rate %" card in KPIs.

- **Documentation** — `docs/api.md`: specification of time series fields (`time_series_10m`) and the `simd_enabled` flag (stats/global). CHANGELOG and SDK examples aligned with the final insertion and search structure.

### Fixed

- **Analytics: query data in Dashboard** — The log buffer was not being read in an up-to-date manner by the analytics endpoint (1h cache). Now uses `entries_10m_fresh()` and `p95_latency_10m_fresh()` for direct file reading when building `time_series_10m`, ensuring Dashboard charts display real latency and throughput.

---

## [Released] - 09/02/2026

### Added

- **Feature: Embedded Model Context Protocol (MCP) support via STDIO.** — MCP server embedded in the FerresDB binary; enabled with the `--mcp` flag or environment variable `FERRESDB_ENABLE_MCP=true`. Exposed tools: `search_points` (vector search with native pre-filtering), `upsert_points` and `get_stats`. The protocol uses stdin/stdout; server logs are redirected to stderr when MCP mode is active. Requires build with the `mcp` feature (`cargo build -p ferres-db-server --features mcp`). Documentation in `docs/api.md` (Model Context Protocol section) and `README.md` (connection with Claude Desktop).

- **Dashboard: Added real-time ingestion throughput and latency charts.** — Analytics page now displays a line chart (Recharts) for ingestion (throughput, points/min in the last 10 min) and an area chart for search latency (ms) in recent queries; KPIs include "Cache Hit Rate %" based on the core's `search_cache`. Backend: `query_log_analytics` with `entries_10m()`, `p95_latency_10m()` and `avg_points_per_second_10m()`; ingestion buffer in `AppState` for time series; `GET /api/v1/stats/analytics` endpoint extended with `time_series_10m` and `cache_hit_rate_pct`. Documentation in `docs/api.md`.

- **Feature: Support for named multi-vector points per document.** — Each point can have a main vector (`vector`) and optionally multiple named vectors (`vectors: HashMap<String, Vec<f32>>`), for example `title_vector` and `content_vector`. Search accepts the `vector_field` parameter to query against the main vector (`default`) or a named field. Separate ANN indices are maintained per vector field; insertion, removal and persistence (JSONL/bincode) support the new structure. Documentation in `docs/api.md`.

- **Storage: Added Zstd compression for WAL and binary snapshot support.** — WAL can use optional Zstd compression (lower disk usage in `wal.log`); point snapshots can be written to `points.bin` (bincode) instead of `points.jsonl`, reducing size and load time. Configuration via `StorageOptions` (core), `wal_compression` / `binary_snapshot` on the server (config.toml or env `FERRESDB_WAL_COMPRESSION`, `FERRESDB_BINARY_SNAPSHOT`). Documentation in `docs/api.md`.

- **Ecosystem: Added foundation for LangChain and LlamaIndex integrations.**

- **Search: Implemented native HNSW pre-filtering for higher accuracy with metadata.** — The filter is applied during graph exploration (nodes that do not satisfy the predicate are ignored before entering the candidate list). The search continues exploring with increasing `ef` until up to `limit` valid results are obtained or the graph is exhausted.

- **Performance: SIMD kernels implemented with hardware status visibility in Dashboard.** — SIMD kernels (AVX2/SSE4.1) for `euclidean_distance` and `dot_product` in `crates/core/src/search.rs` (focus on `QuantizedHnswIndex` SQ8); runtime detection via `simd_enabled()`. Endpoint `GET /api/v1/stats/global` exposes `simd_enabled: bool`; Dashboard (Overview) displays "SIMD Acceleration: Active" or "SIMD: Scalar Fallback" badge.

- **Performance: Added SIMD-accelerated distance kernels (AVX2/SSE).** — Distance kernels (`euclidean_distance`, `dot_product`) in `crates/core/src/search.rs` use the `pulp` crate for safe SIMD abstraction, with runtime dispatch for AVX2 (8× f32) or SSE4.1 (4× f32) and automatic scalar fallback. Asymmetric SQ8 distance (f32×u8) remains optimized in `quantization.rs` (multiple bytes in parallel).

- **Features: Time-to-Live (TTL) support for automatic data expiration**

- **Support for Logical Namespaces (Multitenancy)** — Data isolation per client in the same physical collection via optional `namespace` field in Point. Avoids thousands of collections: multiple tenants share a collection and data is filtered by namespace. Includes: `namespace` field in Point (optional); `MetadataFilter` with first-class `$namespace` condition and `matches_namespace`/`matches_point` methods; composite internal key `(namespace, id)` for uniqueness; persistence in storage and WAL; `namespace` parameter in searches, get point and delete. Documentation in `docs/api.md`.

- **Background Auto-Reindex (worker)** — Background worker that every 30 minutes iterates over collections and checks the tombstone ratio (`tombstone_count / total_indexed`). When the ratio exceeds 20%, it triggers automatic reindex using the existing index swap logic. Detailed start/end cycle and compaction logs via `tracing`. In `crates/core`: `tombstone_ratio()`, `total_indexed_len()` in `Collection`, documentation of `total_indexed` in `needs_reindex`. In `crates/server`: Tokio task in `main.rs`, `run_auto_reindex_cycle()` in `handlers/reindex.rs`, graceful shutdown of the task.

### Performance / Search

- **Optimizations: SIMD-accelerated distance kernels** — Vector distance calculations accelerated with SIMD instructions (AVX2 and SSE4.1) and runtime detection with scalar fallback. f32×f32 distances: `euclidean_distance` and `dot_product` in `search.rs` with AVX2 (8× f32) and SSE4.1 (4× f32) kernels; asymmetric f32×u8 distance (SQ8): `asymmetric_distance` in `quantization.rs` accelerated for `QuantizedHnswIndex` re-ranking. Significant throughput gains for 256–384 dimension vectors on CPUs with AVX2.

- **Native HNSW pre-filtering** — Metadata-filtered search now applies the filter **during** HNSW graph exploration (via `search_filter` from hnsw_rs), instead of searching `limit*10` results and filtering afterwards. Guarantees higher accuracy and consistency in the number of results returned (up to `limit` that satisfy the filter). The `ANNIndex` trait was extended with an optional `predicate` parameter in `search` and `search_explain`; `HnswIndex` and `QuantizedHnswIndex` use native pre-filtering when a predicate is present.

## [Released] - 08/02/2026 - 12:00

### Added

- **Native gRPC API — high-performance alternative to the REST API with bidirectional streaming**
  - Feature flag `grpc` in `crates/server/Cargo.toml` — server works without gRPC by default (REST only).
  - Proto file `crates/server/proto/ferresdb.proto` with package `ferresdb.v1`.
  - `FerresDB` service with 13 RPCs mirroring the REST API:
    - `CreateCollection`, `GetCollection`, `ListCollections`, `DeleteCollection` — collection CRUD.
    - `UpsertPoints`, `DeletePoints`, `GetPoint`, `ListPoints` — point management.
    - `Search`, `HybridSearch`, `ExplainSearch` — vector, hybrid and explain search.
    - `StreamUpsert` (client→server streaming) and `StreamSearch` (bidirectional) — streaming operations.
  - New module `crates/server/src/grpc.rs` (~960 lines): complete gRPC service implementation reusing `AppState`, `Collection`, `Point`, `MetadataFilter` — zero business logic duplication.
  - `build.rs` with `tonic-build` for automatic proto compilation (requires `protoc`).
  - gRPC server (tonic) listens on port 50051 (configurable via `GRPC_PORT` env) in parallel with REST.
  - Optional dependencies: `tonic 0.12`, `prost 0.13`, `tonic-build 0.12`, `async-stream 0.3`.
  - Metadata and filters transmitted as JSON string (`metadata_json`, `filter_json`) in gRPC.
  - `DistanceMetric` mapped to protobuf enum (1=Cosine, 2=DotProduct, 3=Euclidean).
  - Prometheus metrics and query stats registered for gRPC queries (same counters/histograms as REST).
  - Documentation: `docs/api.md` with complete gRPC section (REST→gRPC mapping, `grpcurl` examples, client generation).
  - SDKs: READMEs updated with instructions to generate gRPC stubs in Python, TypeScript and Go.

- **Background Reindex — ANN index rebuild without downtime**
  - New module `crates/core/src/reindex.rs` with all reindex logic in the background.
  - `ReindexJob`, `ReindexStatus`, `ReindexStats`: types for tracking reindex jobs.
  - 3-phase flow: **Building** (separate thread, searches continue on old index), **Swapping** (write lock < 1ms to swap indices), **Cleanup** (drop of old index).
  - `build_new_index()`: builds new `Box<dyn ANNIndex>` from a point snapshot — without tombstones.
  - `apply_delta()`: reconciles insertions/removals that occurred during the build phase.
  - `needs_reindex()`: detects when tombstones > 20% of indexed points.
  - `estimate_index_size()`: estimates index size in bytes.
  - `ANNIndex` trait extended with `tombstone_count()` (implemented in `HnswIndex` and `QuantizedHnswIndex`).
  - `Collection` extended with `tombstone_count()`, `points_snapshot()`, `swap_index()`.
  - New server endpoints:
    - `POST /api/v1/collections/{name}/reindex` — starts reindex job (returns 202 Accepted).
    - `GET /api/v1/collections/{name}/reindex/{job_id}` — job status.
    - `GET /api/v1/collections/{name}/reindex` — lists jobs for the collection.
  - Auto-reindex: after point deletion, if tombstones > 20%, a reindex is automatically triggered.
  - Jobs registered in `AppState` via `DashMap<String, Arc<RwLock<ReindexJob>>>`.
  - Restriction: only 1 reindex per collection at a time (returns 409 if an active job already exists).
  - Tests: `test_reindex_cleans_tombstones`, `test_reindex_concurrent_search`, `test_reindex_with_concurrent_writes`, `test_reindex_job_lifecycle`, `test_reindex_job_failure`, `test_build_new_index`, `test_apply_delta_additions`, `test_apply_delta_removals`, `test_needs_reindex`, `test_estimate_index_size`, `test_reindex_stats_default`, `test_reindex_job_serialization`.
  - Updated SDKs:
    - **Python**: `start_reindex()`, `get_reindex_job()`, `list_reindex_jobs()` + models `ReindexJob`, `ReindexStatus`, `ReindexStats`, `StartReindexResponse`.
    - **TypeScript**: `startReindex()`, `getReindexJob()`, `listReindexJobs()` + corresponding types and Zod schemas.
  - Dashboard: API client with `reindexApi.start()`, `reindexApi.getJob()`, `reindexApi.listJobs()`.
  - Documentation: `docs/api.md` updated with endpoints, schemas and examples.

- **Fusion Strategies for Hybrid Search — Reciprocal Rank Fusion (RRF) as an alternative to weighted score**
  - New module `crates/core/src/fusion.rs` with decoupled fusion algorithms.
  - `FusionStrategy` (enum): `WeightedScore { alpha }` (compatible with original behavior) and `RRF { k }` (pure rank-based fusion).
  - `reciprocal_rank_fusion()`: generic fusion of N rankings via `score = Σ 1/(k + rank_i)`. Supports any number of rankers.
  - `weighted_fusion()`: weighted fusion of 2 rankings (vector + keyword) with alpha. Replicates original behavior.
  - `Collection::hybrid_search()` now accepts `FusionStrategy` instead of `alpha` directly.
  - New optional fields in endpoint `POST /api/v1/collections/{name}/search/hybrid`:
    - `fusion`: `"weighted"` (default) or `"rrf"`.
    - `rrf_k`: k constant for RRF (default: 60).
  - Backward compatible: requests without `fusion` use `"weighted"` with `alpha` (identical behavior to before).
  - Tests: `test_rrf_basic`, `test_rrf_no_overlap`, `test_rrf_vs_weighted`, `test_weighted_backward_compat`, `test_rrf_limit`, `test_rrf_empty_rankings`, `test_rrf_single_ranking`, `test_weighted_fusion_extreme_alpha`, `test_rrf_three_rankers`.
  - Updated SDKs:
    - **Python**: `fusion` and `rrf_k` parameters in `hybrid_search()`.
    - **TypeScript**: `fusion` and `rrf_k` fields in `HybridSearchQuery`.
  - Dashboard: fusion strategy selector in the Hybrid tab of Query Tester.
  - Documentation: `api.md` updated with new parameters, examples and guide on when to use RRF vs weighted.

- **Tiered Storage — automatic movement of vectors between storage tiers based on access frequency**
  - New module `crates/core/src/tiered.rs` with all tiered storage logic.
  - `TieredStorageConfig`: opt-in configuration with Hot/Warm/Cold thresholds and compaction interval.
  - `StorageTier` (enum): `Hot` (RAM), `Warm` (mmap), `Cold` (on-demand disk).
  - `AccessTracker`: tracks last access and count per point for automatic tier decision.
  - `WarmStorage`: vector storage in memory-mapped files via `memmap2`.
  - `ColdStorage`: full point persistence on disk (JSON), loaded on-demand.
  - `TieredCollection`: wrapper over `Collection` that manages Hot/Warm/Cold with automatic promotion and demotion.
  - Background compaction: periodic task that demotes Hot→Warm→Cold points based on access thresholds.
  - Automatic promotion: any access to a Warm/Cold point promotes it to Hot.
  - HNSW graph **always** in memory — only point data is tiered.
  - `CollectionConfig` extended with `tiered_storage` field (`#[serde(default)]` for backward compatibility).
  - `CollectionMeta` in `storage.rs` includes `tiered_storage` for persistence.
  - `FileStorage::save_tier_metadata` / `load_tier_metadata` to persist `TierMetadata` (tiers, accesses).
  - New endpoint `GET /api/v1/collections/{name}/tiers`: returns point distribution per tier and estimated memory.
  - Tests: `test_tier_demotion`, `test_tier_promotion`, `test_search_across_tiers`, `test_compaction`, `test_tier_distribution`, `test_tiered_disabled_everything_hot`, `test_tier_metadata_serialization`, `bench_search_latency_hot_vs_cold`.
  - Dependency: `memmap2 = "0.9"` in workspace.
  - Updated SDKs:
    - **Python**: `TieredStorageConfig` model, `get_tier_distribution()` in client, `TierDistribution` response model.
    - **TypeScript**: `TieredStorageConfig` interface/schema, `getTierDistribution()` in client, `TierDistribution` response type.
  - Documentation: `api.md` updated with new endpoint and tiered storage configuration.

## [Released] - 07/02/2026 - 15:00 - 0.2.0

### Added

- **Real-time Streaming via WebSocket — real-time ingestion and event subscription**
  - New endpoint `GET /api/v1/ws` for HTTP → WebSocket upgrade.
  - JSON protocol over WebSocket with typed messages:
    - `upsert`: real-time point ingestion with automatic batching (10ms debounce).
    - `subscribe`: subscription for events from a collection (`upsert`, `delete`).
    - `ping`/`pong`: application heartbeat.
    - `ack`: operation confirmation with `upserted`, `failed`, `took_ms`.
    - `event`: change notification (collection, action, point_ids, timestamp).
    - `error`: error with message and HTTP code.
  - New `handlers/streaming.rs` with `ws_handler` and `handle_ws_connection`.
  - Automatic batching: accumulates messages for 10ms before flush (debounce) for better throughput.
  - Heartbeat: ping every 30s, disconnection if pong doesn't arrive within 10s.
  - Inactivity timeout: 5 minutes without activity closes the connection.
  - Authentication: accepts API key as query param (`?token=sk-xxx`) or `Authorization: Bearer <key>` header.
  - Configurable limit of 100 simultaneous WebSocket connections.
  - Messages > 10MB automatically rejected.
  - New `CollectionEvent` and `event_channels` (broadcast) in `AppState` for event propagation.
  - REST handlers `upsert_points` and `delete_points` now emit events on the broadcast channel for WebSocket subscribers.
  - Prometheus metrics: `ws_connections_active` (gauge), `ws_messages_received_total` (counter by type), `ws_messages_sent_total` (counter by type).
  - E2E tests: upsert of 10 points via WS, ping/pong, subscribe + event via REST, rejection without auth, invalid message, non-existent collection.
  - Dependencies: `axum` with `ws` feature, `tokio-tungstenite`.

- **Scalar Quantization (SQ8) — vector compression from f32 to u8 with ~4× memory savings**
  - New module `crates/core/src/quantization.rs`: `QuantizationConfig` (enum `None | Scalar`), `ScalarQuantizationConfig` (dtype, always_ram, quantile), `ScalarType::Int8`, `ScalarQuantizationParams` (mins, maxs, scales per dimension).
  - `ScalarQuantizationParams::calibrate()`: calibrates min/max/scale per dimension with percentiles for robustness against outliers. Sample limited to 10K vectors for performance.
  - `ScalarQuantizationParams::quantize()`: maps `f32` → `u8` per dimension (`[min,max]` → `[0,255]`).
  - `ScalarQuantizationParams::dequantize()`: inverse (approximate) operation for reconstruction.
  - `ScalarQuantizationParams::asymmetric_distance()`: asymmetric distance (query f32 vs candidate u8) for Euclidean, Cosine and DotProduct — preserves more precision than quantizing both.
  - New `QuantizedHnswIndex` in `search.rs`: implements `ANNIndex` combining HNSW (for graph navigation) with quantized u8 vectors.
    - `build()`: calibrates params, quantizes vectors, builds HNSW with dequantized vectors.
    - `search()`: expanded HNSW search → re-rank with asymmetric distance → optional re-rank with original f32 (if `always_ram=true`).
    - `add_point()`: quantizes vector, inserts into HNSW with dequantized.
    - `remove_point()`: delegates tombstone to internal HNSW.
  - Factory function `create_ann_index()`: selects `HnswIndex` or `QuantizedHnswIndex` based on config.
  - `CollectionConfig` extended with `quantization: QuantizationConfig` field (`#[serde(default)]` for backward compatibility).
  - `Collection::new()` now uses `create_ann_index()` to create the appropriate index.
  - `CreateCollectionRequest` on the server accepts `quantization` field (opt-in via API).
  - `CollectionMeta` in `storage.rs` includes `quantization` for persistence.
  - Benchmarks in `crates/core/benches/performance.rs`: recall@10 SQ8 vs f32, search latency, memory usage for 10K and 100K vectors.
  - Tests: `test_sq8_calibration`, `test_sq8_roundtrip` (error < 1%), `test_sq8_recall` (recall@10 > 90% vs f32), `test_sq8_memory` (4× compression), `test_quantized_hnsw_basic`, `test_quantized_hnsw_always_ram`, `test_quantized_hnsw_add_point`, `test_quantized_hnsw_remove_point`, `test_create_ann_index_*`, serialization/deserialization tests, tests with outliers.
  - Existing collections without quantization continue to work without change (default: `QuantizationConfig::None`).

- **RBAC (Role-Based Access Control) with granular audit trail**
  - New module `crates/server/src/permissions.rs`: `Resource` (AllCollections, Collection(name)), `Action` (Read, Write, Create, Delete, Admin), `MetadataRestriction`, `Permission`, `PermissionResult`, `check_permission`, `merge_restriction_filter`.
  - New module `crates/server/src/audit.rs`: `AuditEntry`, `AuditResult`, `AuditLogger` (append-only JSONL, daily rotation `audit-YYYY-MM-DD.jsonl`), asynchronous write via `tokio::spawn`.
  - UserStore extended: `permissions TEXT` column (JSON) in SQLite, `ALTER TABLE` migration, `get_permissions`, `update_permissions`, `create_with_permissions`; `UserInfo` with optional `permissions` field.
  - Auth: `AuthUser` with `permissions: Option<Vec<Permission>>`; `AuthenticatedUser` extractor loads permissions from UserStore via state; helper `check_user_permission` (Admin bypasses, legacy role fallback).
  - Enforcement: point handlers (search, search_hybrid, upsert, delete_points, explain_search, estimate_search), collections (create, delete), save, keys, users use `AuthenticatedUser` and verify granular permission; in `search_points` the `MetadataRestriction` is injected into the filter (AND with request).
  - New error `ApiError::Forbidden` (403).
  - Endpoint `GET /api/v1/audit` (Admin only): query params `user`, `action`, `resource`, `from`, `to`, `limit`; returns filtered audit entries.
  - Endpoint `PUT /api/v1/users/{username}/permissions` to update granular permissions (Admin).
  - Instrumentation of all handlers with audit trail (login, search, upsert, delete_points, create/delete collection, save, create/delete API key, create/delete user, update password/permissions).
  - Documentation in `docs/api.md`: RBAC model, permissions and audit endpoints, enforcement per endpoint.
  - Tests in `crates/server/tests/rbac_test.rs`: viewer cannot upsert, viewer can search, admin bypasses restrictions, MetadataRestriction filters results, per-collection permissions, granular write, audit records actions and denials, audit requires Admin; unit tests in `permissions` and `audit`, permissions persistence in UserStore.

## [Released] - 07/02/2026 - 11:00 - 0.1.1

### Added

- **Query Cost Estimation — query cost estimation before execution**
  - New module `crates/core/src/cost.rs` with types: `QueryCostEstimate`, `CostBreakdown`, `CostEstimateParams`.
  - Function `estimate_search_cost` with HNSW-based heuristics (O(log n × ef_search × dimension)), post-search filter cost, hydration and network overhead.
  - New method `VectorDB::estimate_query_cost` — estimates cost without executing a real search.
  - New endpoint `POST /api/v1/collections/{name}/search/estimate` — returns latency estimate, memory, visited nodes, `is_expensive` flag and optimization recommendations.
  - Optional field `include_history` to include historical percentiles (p50/p95/p99) in the response.
  - **Budget-Based Queries**: optional `budget_ms` field in the `POST /search` endpoint. If the estimate exceeds the budget, returns `422 Unprocessable Entity` with the detailed estimate in the body (without executing the search).
  - New error `BudgetExceeded` (422) on the server with cost estimate in the body.
  - Automatic recommendations: high limit, filters on large collections, high-dimension vectors, high ef_search.
  - Unit tests in `cost.rs` (15 tests: positive values, filter, size, limit, is_expensive, recommendations, serialization, etc.).
  - E2E tests: estimate endpoint, collection not found, budget rejected, budget accepted.
  - Documentation of endpoint and budget_ms in `docs/api.md`.

- **Explain Query — detailed explanation of vector search results**
  - New module `crates/core/src/explain.rs` with types: `SearchExplanation`, `ExplainResult`, `FilterExplanation`, `ConditionResult`, `ExplainMeta`, `IndexStats`.
  - New method `search_explain` in the `ANNIndex` trait (with default implementation and optimized override in `HnswIndex`).
  - New method `search_explain` in `Collection` (delegates to index).
  - New method `VectorDB::search_explain` — vector search with complete explanation: score breakdown, per-condition filter evaluation, ranking before/after filters and index statistics.
  - New endpoint `POST /api/v1/collections/{name}/search/explain` on the HTTP server.
  - Helper `evaluate_condition` to individually evaluate each filter condition against metadata.
  - Unit tests for explain (basic, with filter, with BM25 enabled, condition evaluation).
  - Documentation of endpoint in `docs/api.md`.

- **OpenTelemetry Distributed Tracing — complete distributed tracing of each vector search**
  - Enhanced `crates/server/src/tracing_otel.rs`: explicit reading of `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`), registration of the global W3C Trace Context propagator (`traceparent`/`tracestate`).
  - Enriched spans in `search_points` and `search_hybrid` with 9+ OTel attributes: `db.collection`, `db.operation`, `db.vector.dimension`, `db.vector.limit`, `db.results.count`, `db.duration.search_ms`, `db.duration.hydrate_ms`, `db.index.type`, `db.index.ef_search`, `db.hybrid.alpha`. Values filled via `Span::current().record()`.
  - Granular child spans in core: `collection.search` (attributes: `points`, `dimension`) in `Collection::search()` and `hnsw.search` (attributes: `candidates`, `ef`, `tombstones`) in `HnswIndex::search()`.
  - W3C trace context propagation in `request_logger` middleware: extraction of `traceparent`/`tracestate` from HTTP headers, linking to `http_request` span, and inclusion of `x-trace-id` response header for debugging.
  - All OTel code protected by `#[cfg(feature = "otel")]` — server works normally without the feature.
  - "Observability" section in `README.md`: how to enable, environment variables, example with Jaeger, span hierarchy, OTel attributes table.

### Fixed

- **Performance: early release of read lock in search handlers**
  - `search_points`: the `RwLockReadGuard` and `DashMap Ref` are now dropped immediately after result hydration (`collection.get()`), before metadata filtering, `QueryProfile` construction, `DashMap` insertion, Prometheus metrics, `query_stats` and `query_logger` spawn.
  - `search_hybrid`: same pattern — lock released right after hydrating hybrid search results (vector + BM25).
  - Impact: reduces `RwLock` contention, allowing concurrent writes (upsert/delete) to not be blocked by operations that do not need the lock (metrics, logging, profiling).

---

## 2025-W05 (week of Feb 03–09 2025)

### Added

- **REST API (HTTP server)**
  - `GET /health` — health check
  - `GET /metrics` — Prometheus metrics
  - `POST /api/v1/save` — persist all collections to disk
  - `POST /api/v1/collections` — create collection (name, dimension, distance, enable_bm25, bm25_text_field)
  - `GET /api/v1/collections` — list collections
  - `GET /api/v1/collections/{name}` — collection details
  - `DELETE /api/v1/collections/{name}` — remove collection
  - `POST /api/v1/collections/{name}/points` — upsert points (up to 1000 per request)
  - `DELETE /api/v1/collections/{name}/points` — remove points by IDs
  - `GET /api/v1/collections/{name}/points/{id}` — get point by ID
  - `POST /api/v1/collections/{name}/search` — vector search (vector, limit, filter)
  - `POST /api/v1/collections/{name}/search/hybrid` — hybrid search (vector + BM25, RRF)
  - `GET /api/v1/collections/{name}/stats` — statistics (num_points, num_queries, latencies)
- **Core**
  - Hybrid search (vector + BM25) with RRF; support for `enable_bm25` and `bm25_text_field` on collection creation
  - Metadata filtering in vector search (equality)
- **Rust SDK (ferres-db-sdk)**
  - `FerresDbClient::new(base_url)` and `hybrid_search(collection, query_text, query_vector, limit, alpha)`
  - Types `HybridSearchResponse`, `SearchResultItem`, `SdkError`
- **Documentation**
  - [docs/api.md](docs/api.md) — HTTP API reference with curl and JSON schemas
  - [docs/sdk.md](docs/sdk.md) — Rust SDK guide and API usage in Python/TypeScript
  - Root README: overview, architecture diagram (Mermaid), quick start in 3 steps, links to docs
  - [examples/simple_rag/README.md](examples/simple_rag/README.md) — step-by-step tutorial, troubleshooting, next steps
  - [CHANGELOG.md](CHANGELOG.md) — change log by week

### Changed

- N/A (initial entry)

### Fixed

- N/A (initial entry)

---

## How to use this changelog

- **Unreleased**: items already implemented but not yet published in a release.
- **By week**: use the format `## YYYY-Wxx (week of DD–DD Mon YYYY)` or `## DD/MM/YYYY - DD/MM/YYYY` and group changes under **Added**, **Changed**, **Fixed** (and optionally **API**, **Deprecated**, **Removed**, **Security**).
