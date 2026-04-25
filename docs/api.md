# HTTP API Reference — FerresDB

Reference for REST endpoints of the FerresDB server. Example base URL: `http://localhost:8080`.

## CPU Requirements for Maximum Performance

For best throughput in vector searches, the server uses SIMD kernels when available. A CPU with **AVX2** support, or at least **SSE4.1**, is recommended for maximum performance on distance operations (Euclidean, DotProduct, Cosine) and in SQ8 quantization re-ranking. On CPUs without these instructions, the code uses a scalar implementation (correct behavior, with lower performance). Detection is automatic at runtime; no configuration is required.

**Technical note (hardware):** Distance kernels (`euclidean_distance`, `dot_product`) and asymmetric distance for SQ8 are accelerated by **AVX2** instructions (8-float vectors) or **SSE4.1** (4 floats), with automatic scalar fallback. For maximum performance in production, use processors that support at least AVX2 (Intel Haswell or later, AMD Excavator/Zen or later). In environments without these extensions (e.g. some VMs or older CPUs), behavior remains correct with reduced throughput.

**Vector quantization:** FerresDB supports two vector compression strategies for memory reduction (~4× less than `f32`):

- **SQ8** (`Scalar`): maps each `f32` dimension → `u8` via percentile calibration per block. Requires a calibration step on existing data.
- **PolarQuant** (`Polar`): converts pairs of Cartesian coordinates into `(radius, angle)` recursively. Angles are always in `[0, 2π]` — no per-block calibration parameters. Configurable via `bits_per_angle` (default: 8).

---

## Conventions

- **Content-Type:** `application/json` for requests with body.
- **Authentication:** Protected routes accept API key in the `Authorization: Bearer <api-key>` header (see API Keys and auth section). The official benchmark tool (`ferres-bench`) supports `--api-key` and env `FERRESDB_API_KEY`; see [benchmark.md](benchmark.md).
- **Errors:** Error responses use the schema below and the appropriate HTTP status.

### Error schema

```json
{
  "error": "string (error type)",
  "message": "string (description)",
  "code": 400
}
```

| Field     | Type   | Description                                                                                                                                                                     |
| --------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `error`   | string | Type: `collection_not_found`, `collection_already_exists`, `invalid_payload`, `invalid_dimension`, `internal_error`, `query_profile_not_found`, `method_not_allowed` (replica) |
| `message` | string | Human-readable message                                                                                                                                                          |
| `code`    | number | HTTP code (400, 404, 409, 500)                                                                                                                                                  |

---

## Health

### GET /health

Checks whether the server is running.

**Response:** `200 OK`

**Response schema:**

```json
{
  "status": "OK"
}
```

**curl example:**

```bash
curl -s http://localhost:8080/health
```

---

## Metrics (Prometheus)

### GET /metrics

Returns metrics in Prometheus format (text/plain).

**Response:** `200 OK` (text body)

**curl example:**

```bash
curl -s http://localhost:8080/metrics
```

---

## Persistence

### POST /api/v1/save

Persists all collections to disk. Useful before restarting the server or in persistence tests.

**Request:** No body (or `{}`).

**Response:** `200 OK`

**Response schema:**

```json
{
  "ok": true
}
```

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/save
```

### Storage configuration parameters

The server and core support options to reduce disk usage and load time:

| Parameter                      | Type    | Default | Description                                                                                                                                                                                                                                       |
| ------------------------------ | ------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `wal_compression`              | boolean | false   | Compresses the Write-Ahead Log (wal.log) with Zstd. Reduces WAL size on disk. Active when the server uses VectorDB with WAL.                                                                                                                     |
| `binary_snapshot`              | boolean | false   | Writes point snapshots in binary format (points.bin with bincode) instead of JSONL (points.jsonl). Reduces file size and speeds up loading. Loading automatically detects the format (points.bin or points.jsonl).                               |
| `namespace_physical_isolation` | boolean | false   | Physical isolation per namespace: points with `namespace` are written to `data/collections/<name>/namespaces/<namespace>/points.bin`. Allows snapshot and cleanup per tenant without affecting others.                                            |

**Directory layout with physical namespace isolation**

When `namespace_physical_isolation` is active:

- Points **without** namespace: `data/collections/<collection_name>/points.bin` (or `points.jsonl`) and `index.bin`.
- Points **with** namespace: `data/collections/<collection_name>/namespaces/<namespace_name>/points.bin` (and optionally `index.bin`, `checksum.md5`).

Loading detects the existence of `namespaces/` and loads points from each subdirectory, rebuilding the index from the unified set. This allows per-namespace snapshot operations (copy/delete only `data/collections/<name>/namespaces/<tenant_id>/`) and physical data cleanup for one tenant without affecting others.

**Server configuration**

- **config.toml file** (at project root or in `../`, `../../`):

```toml
wal_compression = false
binary_snapshot = true
namespace_physical_isolation = false
```

- **Environment variables** (override TOML):
  - `FERRESDB_WAL_COMPRESSION` — `true` or `1` to enable WAL compression.
  - `FERRESDB_BINARY_SNAPSHOT` — `true` or `1` to enable binary snapshots.
  - `FERRESDB_NAMESPACE_PHYSICAL_ISOLATION` — `true` or `1` to enable physical namespace isolation (multitenancy).

**Usage in core (VectorDB)**

Use `VectorDB::with_storage_options(path, options)` with `StorageOptions { wal_compression: true, binary_snapshot: true, namespace_physical_isolation: true }` as needed when using core directly.

### POST /api/v1/admin/backup

Generates a binary snapshot (tar.gz) of the entire storage directory (collections, API keys, users; excludes `logs`) and uploads it to the configured S3 bucket. Requires authentication and **Admin role**.

**Request:** No body (or `{}`).

**Response:** `200 OK` on success.

**Response schema:**

```json
{
  "ok": true,
  "key": "backups/ferresdb-2026-02-09T12-00-00Z.tar.gz",
  "bucket": "my-backups",
  "size_bytes": 1048576,
  "region": "us-east-1"
}
```

| Field        | Type    | Description                    |
| ------------ | ------- | ------------------------------ |
| `ok`         | boolean | Always `true` on success.      |
| `key`        | string  | Object key in S3.              |
| `bucket`     | string  | Bucket name.                   |
| `size_bytes` | number  | Archive size in bytes.         |
| `region`     | string  | AWS region used (optional).    |

**Errors:** `503` if S3 is not configured (`s3_region`/`s3_bucket`); `502` if S3 upload fails; `500` if archive creation or collection save fails.

**curl example:**

```bash
curl -s -X POST -H "Authorization: Bearer YOUR_JWT_OR_API_KEY" http://localhost:8080/api/v1/admin/backup
```

### S3 Configuration (cloud backup)

| Parameter              | Type   | Default | Description                                                                                                  |
| ---------------------- | ------ | ------- | ------------------------------------------------------------------------------------------------------------ |
| `s3_region`            | string | —       | AWS region (e.g. `us-east-1`). Env: `FERRESDB_S3_REGION` or `AWS_REGION`.                                   |
| `s3_bucket`            | string | —       | S3 bucket name. Env: `FERRESDB_S3_BUCKET`.                                                                   |
| `s3_access_key_id`     | string | —       | Access Key ID (optional; can use AWS variables). Env: `FERRESDB_S3_ACCESS_KEY_ID` or `AWS_ACCESS_KEY_ID`.    |
| `s3_secret_access_key` | string | —       | Secret Access Key (optional). Env: `FERRESDB_S3_SECRET_ACCESS_KEY` or `AWS_SECRET_ACCESS_KEY`.               |

**config.toml (optional):**

```toml
s3_region = "us-east-1"
s3_bucket = "ferres-backups"
# Credentials: prefer environment variables (FERRESDB_S3_ACCESS_KEY_ID, FERRESDB_S3_SECRET_ACCESS_KEY or AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
```

Credentials can be omitted from the file (recommended) and set only via environment variables or the system AWS profile.

### Point-in-Time Recovery (PITR) and disaster recovery

FerresDB supports **Point-in-Time Recovery (PITR)** using the Write-Ahead Log (WAL) with timestamps. Each WAL entry has a Unix timestamp; when saving a snapshot, the server persists the timestamp in `last_snapshot_timestamp` in the collection directory. This allows restoring a collection (or all of them) to the state at a past point in time.

**PITR flow:**

1. Load the last snapshot on disk (state at the time of the last `save`).
2. Read `wal.log` entries and replay only those with `timestamp > last_snapshot_timestamp` and `timestamp <= target_timestamp`.
3. Replace the in-memory collection with the resulting state, persist to disk and truncate the WAL.

**When to use:** After a human error (e.g. mass delete), partial corruption, or to audit the state at a past point in time. It is recommended to combine with regular backups (e.g. `POST /api/v1/admin/backup` to S3) for disaster recovery that affects the entire disk.

#### GET /api/v1/admin/restore/points

Lists available restore points per collection: last snapshot timestamp and list of WAL entry timestamps. Requires authentication and **Admin role**.

**Query params (optional):**

| Parameter    | Type   | Description                                  |
| ------------ | ------ | -------------------------------------------- |
| `collection` | string | If present, restricts to the indicated collection. |

**Response:** `200 OK`

**Response schema:**

```json
{
  "collections": {
    "my_collection": {
      "last_snapshot_timestamp": 1739182800,
      "wal_timestamps": [1739182810, 1739182820, 1739182830]
    }
  }
}
```

Use these timestamps as the target in `POST /api/v1/admin/restore`.

**curl example:**

```bash
curl -s -H "Authorization: Bearer YOUR_ADMIN_KEY" "http://localhost:8080/api/v1/admin/restore/points"
curl -s -H "Authorization: Bearer YOUR_ADMIN_KEY" "http://localhost:8080/api/v1/admin/restore/points?collection=my_collection"
```

#### POST /api/v1/admin/restore

Restores one or all collections to the state at the given timestamp (Point-in-Time Recovery). Requires authentication and **Admin role**.

**Request body:**

| Field        | Type   | Required | Description                                                              |
| ------------ | ------ | -------- | ------------------------------------------------------------------------ |
| `timestamp`  | number | yes      | Unix timestamp in seconds to restore to.                                 |
| `collection` | string | no       | If present, restores only this collection; otherwise, restores all.      |

**Request schema:**

```json
{
  "timestamp": 1739182800,
  "collection": "my_collection"
}
```

**Response:** `200 OK` (or `422` if all restorations fail)

**Response schema:**

```json
{
  "ok": true,
  "restored": ["my_collection"],
  "errors": []
}
```

| Field      | Type     | Description                                           |
| ---------- | -------- | ----------------------------------------------------- |
| `ok`       | boolean  | `true` if no errors occurred.                         |
| `restored` | string[] | Names of successfully restored collections.           |
| `errors`   | string[] | Error messages per collection (e.g. not found).       |

After restoration, the on-disk and in-memory state becomes the state at `timestamp`; the WAL is truncated to reflect that there are no pending operations after that point.

**curl example:**

```bash
# Restore only the "docs" collection to its state at 12:00 UTC on 2025-02-10
curl -s -X POST -H "Content-Type: application/json" -H "Authorization: Bearer YOUR_ADMIN_KEY" \
  http://localhost:8080/api/v1/admin/restore \
  -d '{"timestamp":1739182800,"collection":"docs"}'

# Restore all collections to a point in time
curl -s -X POST -H "Content-Type: application/json" -H "Authorization: Bearer YOUR_ADMIN_KEY" \
  http://localhost:8080/api/v1/admin/restore \
  -d '{"timestamp":1739182800}'
```

**Dashboard:** The **Snapshots & Recovery** tab allows viewing restore points per collection and triggering a restore by entering the timestamp (and optionally the collection).

---

## Collections

### POST /api/v1/collections

Creates a new collection.

**Request body:**

| Field             | Type    | Required | Description                                                                                                                      |
| ----------------- | ------- | -------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `name`            | string  | yes      | Unique name: only `a-zA-Z0-9_-`                                                                                                  |
| `dimension`       | number  | yes      | Vector dimension (1–4096)                                                                                                        |
| `distance`        | string  | yes      | Metric: `Cosine`, `Euclidean`, `DotProduct`                                                                                      |
| `enable_bm25`     | boolean | no       | Enables BM25 index for hybrid search (default: false)                                                                            |
| `bm25_text_field` | string  | no       | Metadata key used as text for BM25 (default: `"text"`)                                                                           |
| `quantization`    | object  | no       | Vector compression. `"None"` (default), `{"Scalar":{"dtype":"Int8"}}` (SQ8) or `{"Polar":{"bits_per_angle":8}}` (PolarQuant)     |
| `tiered_storage`  | object  | no       | Tiered storage configuration (see Tiered Storage section)                                                                        |
| `retention_days`  | number  | no       | Retention in days (WAL and history); omitted or null = no limit                                                                  |

**Request schema:**

```json
{
  "name": "my_collection",
  "dimension": 384,
  "distance": "Cosine",
  "enable_bm25": false,
  "bm25_text_field": "text",
  "quantization": "None"
}
```

**`quantization` examples:**

```json
// No quantization (default)
"quantization": "None"

// Scalar Quantization SQ8 — compresses f32 → u8 per dimension (~4× less memory, requires calibration)
"quantization": {"Scalar": {"dtype": "Int8", "always_ram": false, "quantile": 99.5}}

// PolarQuant — recursive polar coordinates (~4× less memory, no per-block calibration)
"quantization": {"Polar": {"bits_per_angle": 8}}
```

**Response:** `201 Created`

**Response schema:**

```json
{
  "name": "my_collection",
  "dimension": 384,
  "distance": "Cosine",
  "created_at": 1707123456
}
```

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections \
  -H "Content-Type: application/json" \
  -d '{"name":"docs","dimension":384,"distance":"Cosine","enable_bm25":true}'
```

---

### GET /api/v1/collections

Lists all collections. Optionally restricts to collections that have at least one point in the indicated namespace.

**Query params:**

| Param       | Type   | Description                                                                                          |
| ----------- | ------ | ---------------------------------------------------------------------------------------------------- |
| `namespace` | string | When set, returns only collections that have at least one point in this namespace (multitenancy). |

**Response:** `200 OK`

**Response schema:**

```json
{
  "collections": [
    {
      "name": "docs",
      "dimension": 384,
      "num_points": 42,
      "created_at": 1707123456,
      "retention_days": 30
    }
  ]
}
```

The `retention_days` field only appears when set (number of days) or can be omitted when there is no limit.

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/collections
curl -s "http://localhost:8080/api/v1/collections?namespace=tenant-a"
```

---

### GET /api/v1/collections/{name}

Returns details of a collection.

**Path:** `name` — collection name.

**Response:** `200 OK`

**Response schema:**

```json
{
  "name": "docs",
  "dimension": 384,
  "num_points": 42,
  "last_updated": 1707123456,
  "stats": {
    "index_size_bytes": 64512
  },
  "retention_days": 30
}
```

The `retention_days` field is optional (present when configured; null or omitted = no limit).

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs
```

---

### PATCH /api/v1/collections/{name}

Updates the collection retention configuration. Requires authentication and **Admin** permission.

**Path:** `name` — collection name.

**Request body:**

| Field            | Type           | Description                                                     |
| ---------------- | -------------- | --------------------------------------------------------------- |
| `retention_days` | number or null | Retention in days; null or omitted = keep indefinitely.         |

**Response:** `204 No Content` (no body)

The retention worker compacts the collection WAL every hour, removing entries older than `retention_days` days. The change is persisted in `config.json` on disk.

**curl example:**

```bash
curl -s -X PATCH http://localhost:8080/api/v1/collections/docs \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"retention_days": 30}'
```

---

### DELETE /api/v1/collections/{name}

Removes a collection and its data from disk.

**Path:** `name` — collection name.

**Response:** `204 No Content` (no body)

**curl example:**

```bash
curl -s -X DELETE http://localhost:8080/api/v1/collections/docs
```

---

### GET /api/v1/collections/{name}/tiers

Returns the point distribution per storage tier (Tiered Storage).

**Path:** `name` — collection name.

**Response:** `200 OK`

**Response schema:**

```json
{
  "hot": 1000,
  "warm": 5000,
  "cold": 20000,
  "hot_memory_bytes": 1536000,
  "warm_memory_bytes": 1320000,
  "cold_memory_bytes": 1280000
}
```

| Field               | Type   | Description                                           |
| ------------------- | ------ | ----------------------------------------------------- |
| `hot`               | number | Points in the Hot tier (RAM, instant access)          |
| `warm`              | number | Points in the Warm tier (mmap, fast access)           |
| `cold`              | number | Points in the Cold tier (disk, loaded on-demand)      |
| `hot_memory_bytes`  | number | Estimated memory used by the Hot tier (bytes)         |
| `warm_memory_bytes` | number | Estimated memory used by the Warm tier (bytes)        |
| `cold_memory_bytes` | number | Estimated memory used by the Cold tier (bytes)        |

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/tiers \
  -H "Authorization: Bearer <api-key>"
```

> **Note:** When tiered storage is not enabled on the collection, all points
> are reported in the Hot tier. Enable via `tiered_storage` at collection creation.

---

### Tiered Storage (Configuration)

Tiered Storage is configured via the `tiered_storage` field at collection creation:

```json
{
  "name": "my_collection",
  "dimension": 384,
  "distance": "Cosine",
  "tiered_storage": {
    "enabled": true,
    "hot_threshold_hours": 24,
    "warm_threshold_hours": 168,
    "compaction_interval_secs": 3600
  }
}
```

| Field                      | Type    | Default | Description                                                |
| -------------------------- | ------- | ------- | ---------------------------------------------------------- |
| `enabled`                  | boolean | false   | Enables tiered storage                                     |
| `hot_threshold_hours`      | number  | 24      | Points accessed in the last N hours stay in Hot (RAM)      |
| `warm_threshold_hours`     | number  | 168     | Points accessed in the last N hours stay in Warm (mmap)    |
| `compaction_interval_secs` | number  | 3600    | Interval between automatic compactions (seconds)           |

**Tiers:**

| Tier | Storage                   | Latency  | Memory  |
| ---- | ------------------------- | -------- | ------- |
| Hot  | Full RAM                  | ~0ms     | High    |
| Warm | mmap (vector) + RAM (meta)| ~1ms     | Medium  |
| Cold | Disk (on-demand)          | ~5–10ms  | Minimal |

**Behavior:**

- The HNSW graph **always** stays in memory — only point data is tiered.
- Any access to a Cold/Warm point automatically promotes it to Hot.
- Compaction runs in the background without blocking searches.
- When disabled (default), everything stays in RAM as before.

---

## Reindex (Background Index Rebuild)

Rebuilds the ANN index of a collection in the background without blocking searches or mutations. Removes accumulated tombstones and restores search performance.

**Flow:**

1. **Building**: Snapshot of points → new index built in a separate thread. Searches continue on the old index.
2. **Swapping**: Write lock < 1ms to swap old index for the new one. Delta (ops during build) is applied.
3. **Cleanup**: Old index is discarded (memory freed).

**Auto-reindex**: Triggers automatically when tombstones > 20% of indexed points (after deletions).

### POST /api/v1/collections/{name}/reindex

Starts a background reindex job. Only 1 active job per collection.

**Response:** `202 Accepted`

```json
{
  "job_id": "550e8400-e29b-41d4-a716-446655440000",
  "collection": "my-vectors",
  "status": "Building",
  "message": "reindex job started"
}
```

| Field        | Type   | Description                    |
| ------------ | ------ | ------------------------------ |
| `job_id`     | string | UUID of the created job        |
| `collection` | string | Collection name                |
| `status`     | string | Initial status (`Building`)    |
| `message`    | string | Descriptive message            |

**Errors:**

- `404` — collection not found.
- `409` — an active reindex job already exists for this collection.

**Example:**

```bash
curl -X POST http://localhost:8080/api/v1/collections/my-vectors/reindex \
  -H "Authorization: Bearer sk-xxx"
```

### GET /api/v1/collections/{name}/reindex/{job_id}

Returns the status of a specific reindex job.

**Response:** `200 OK`

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "collection": "my-vectors",
  "status": "Completed",
  "progress": 1.0,
  "started_at": 1707300000,
  "completed_at": 1707300042,
  "error": null,
  "stats": {
    "points_processed": 50000,
    "points_total": 50000,
    "tombstones_cleaned": 12500,
    "old_index_size_bytes": 76800000,
    "new_index_size_bytes": 76800000
  }
}
```

| Field                        | Type    | Description                                               |
| ---------------------------- | ------- | --------------------------------------------------------- |
| `id`                         | string  | Job UUID                                                  |
| `collection`                 | string  | Collection name                                           |
| `status`                     | string  | `Queued`, `Building`, `Swapping`, `Completed`, `Failed`   |
| `progress`                   | number  | Progress from 0.0 to 1.0                                  |
| `started_at`                 | number  | UNIX timestamp (seconds)                                  |
| `completed_at`               | number? | Completion timestamp (null if still in progress)          |
| `error`                      | string? | Error message (null on success)                           |
| `stats.points_processed`     | number  | Points processed                                          |
| `stats.points_total`         | number  | Total points in snapshot                                  |
| `stats.tombstones_cleaned`   | number  | Tombstones removed                                        |
| `stats.old_index_size_bytes` | number  | Estimated size of old index (bytes)                       |
| `stats.new_index_size_bytes` | number  | Estimated size of new index (bytes)                       |

**Errors:**

- `404` — collection or job not found.

**Example:**

```bash
curl http://localhost:8080/api/v1/collections/my-vectors/reindex/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer sk-xxx"
```

### GET /api/v1/collections/{name}/reindex

Lists all reindex jobs for a collection (most recent first).

**Response:** `200 OK`

```json
{
  "jobs": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "collection": "my-vectors",
      "status": "Completed",
      "progress": 1.0,
      "started_at": 1707300000,
      "completed_at": 1707300042,
      "error": null,
      "stats": {
        "points_processed": 50000,
        "points_total": 50000,
        "tombstones_cleaned": 12500,
        "old_index_size_bytes": 76800000,
        "new_index_size_bytes": 76800000
      }
    }
  ]
}
```

**Errors:**

- `404` — collection not found.

**Example:**

```bash
curl http://localhost:8080/api/v1/collections/my-vectors/reindex \
  -H "Authorization: Bearer sk-xxx"
```

---

## Points

### POST /api/v1/collections/{name}/points

Inserts or updates points in batch (up to 1000 points per request).

**Path:** `name` — collection name.

**Request body:**

| Field    | Type  | Description                              |
| -------- | ----- | ---------------------------------------- |
| `points` | array | List of 1 to 1000 points (object below). |

Each element of `points`:

| Field       | Type   | Required | Description                                                                                                                                                            |
| ----------- | ------ | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `id`        | string | yes      | Unique point ID                                                                                                                                                        |
| `vector`    | array  | yes      | Main vector (float array), dimension equal to the collection's                                                                                                         |
| `metadata`  | object | no       | Arbitrary JSON (default: `{}`)                                                                                                                                         |
| `namespace` | string | no       | Logical namespace (multitenancy). When omitted, the point has no namespace.                                                                                            |
| `ttl`       | number | no       | TTL in seconds; if present, the point expires after that time and is removed by the vacuum worker.                                                                     |
| `vectors`   | object | no       | Additional named vectors: map of name → float array (e.g. `"title_vector"`, `"content_vector"`). Each vector must have the same dimension as the collection.           |

**Request schema (single vector):**

```json
{
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "metadata": { "text": "Document content" },
      "ttl": 3600
    }
  ]
}
```

**Request schema (multiple vectors per point):**

```json
{
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "vectors": {
        "title_vector": [0.2, 0.1, 0.0],
        "content_vector": [0.0, -0.1, 0.3]
      },
      "metadata": { "text": "Document content" }
    }
  ]
}
```

**Response:** `200 OK`

**Response schema:**

```json
{
  "upserted": 1,
  "failed": [
    { "id": "doc-2", "reason": "dimension mismatch: expected 384, got 128" }
  ]
}
```

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/points \
  -H "Content-Type: application/json" \
  -d '{"points":[{"id":"doc-1","vector":[0.1,0.2,-0.1],"metadata":{"text":"Hello"}}]}'
```

---

### DELETE /api/v1/collections/{name}/points

Removes points by ID.

**Path:** `name` — collection name.

**Request body:**

| Field       | Type   | Required | Description                                                                    |
| ----------- | ------ | -------- | ------------------------------------------------------------------------------ |
| `ids`       | array  | yes      | List of IDs of points to remove                                                |
| `namespace` | string | no       | When provided, removes only points from this namespace (for the given IDs).    |

```json
{
  "ids": ["doc-1", "doc-2"],
  "namespace": "tenant-a"
}
```

**Response:** `200 OK`

**Response schema:**

```json
{
  "deleted": 2
}
```

**curl example:**

```bash
curl -s -X DELETE http://localhost:8080/api/v1/collections/docs/points \
  -H "Content-Type: application/json" \
  -d '{"ids":["doc-1","doc-2"]}'
```

---

### GET /api/v1/collections/{name}/points/{id}

Returns a point by ID.

**Path:** `name` — collection name; `id` — point ID.

**Query params:**

| Field       | Type   | Description                                                                |
| ----------- | ------ | -------------------------------------------------------------------------- |
| `namespace` | string | When the point was inserted with a namespace, provide it for lookup.       |

**Response:** `200 OK`. Includes the `namespace` field when the point has a namespace and `relations` when the point has relations (graph).

**Response schema:**

```json
{
  "id": "doc-1",
  "vector": [0.1, 0.2, -0.1],
  "metadata": { "text": "Content" },
  "created_at": 1707123456,
  "namespace": "tenant-a",
  "relations": ["doc-2", "doc-3"]
}
```

| Field       | Type          | Description                                                              |
| ----------- | ------------- | ------------------------------------------------------------------------ |
| `relations` | string array  | IDs of related points (undirected graph). Omitted if empty.              |

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/points/doc-1
```

---

### POST /api/v1/collections/{name}/points/link

Creates an undirected relation between two points (graph persistence). Updates the `relations` field on both points: the `from` point now includes `to` in its list and the `to` point now includes `from`. Disk persistence happens on the next snapshot (save).

**Path:** `name` — collection name.

**Request body:**

| Field  | Type   | Required | Description                                                                              |
| ------ | ------ | -------- | ---------------------------------------------------------------------------------------- |
| `from` | string | yes      | Origin point ID (storage_id; for points without namespace, the logical point ID).        |
| `to`   | string | yes      | Destination point ID (storage_id).                                                       |

```json
{
  "from": "id_A",
  "to": "id_B"
}
```

**Response:** `200 OK`

**Response schema:**

```json
{
  "ok": true,
  "from": "id_A",
  "to": "id_B"
}
```

**Errors:** `404` if collection or one of the points does not exist; `400` if `from` and `to` are equal or empty; `403` if write permission on the collection is missing.

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/points/link \
  -H "Content-Type: application/json" \
  -d '{"from":"id_A","to":"id_B"}'
```

---

### GET /api/v1/collections/{name}/graph/subgraph

Returns a subgraph of the collection for visualization (e.g. Graph Explorer in the dashboard). Return format: `{ "nodes": [...], "edges": [...] }`.

**Path:** `name` — collection name.

**Query params:**

| Field       | Type   | Required | Description                                                                                            |
| ----------- | ------ | -------- | ------------------------------------------------------------------------------------------------------ |
| `center_id` | string | no       | Subgraph center; used with `depth` for BFS (takes precedence over `seed` when both are present).       |
| `depth`     | number | no       | Hop depth for BFS from `center_id` (e.g. 2 = up to 2 hops).                                           |
| `seed`      | string | no       | If provided (and without center_id), returns the node and its neighbors (1-hop).                       |
| `limit`     | number | no       | Without center_id/seed: maximum number of nodes (default 500, max 2000).                               |

**Response:** `200 OK`

**Response schema:**

```json
{
  "nodes": [
    {
      "id": "id_A",
      "metadata": {},
      "namespace": null,
      "created_at": 1707123456,
      "relations": ["id_B", "id_C"]
    }
  ],
  "edges": [
    { "source": "id_A", "target": "id_B" },
    { "source": "id_A", "target": "id_C" }
  ]
}
```

Each node includes `id` (storage_id), `metadata`, `namespace`, `created_at`, `relations`. The `vector` field is omitted to reduce payload. Use GET `/api/v1/collections/{name}/points/{id}` for full point details.

**curl example:**

```bash
# BFS subgraph (center_id + depth)
curl -s "http://localhost:8080/api/v1/collections/docs/graph/subgraph?center_id=id_A&depth=2"

# 1-hop subgraph (seed)
curl -s "http://localhost:8080/api/v1/collections/docs/graph/subgraph?seed=id_A"

# Full subgraph (points with relations, up to 500 nodes)
curl -s "http://localhost:8080/api/v1/collections/docs/graph/subgraph"
```

---

### POST /api/v1/collections/{name}/search

Searches for the most similar points to the query vector (vector search).

**Path:** `name` — collection name.

**Request body:**

| Field          | Type    | Required | Description                                                                                                                                                                                                                                                                                            |
| -------------- | ------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `vector`       | array   | yes      | Query vector (same dimension as the collection)                                                                                                                                                                                                                                                        |
| `limit`        | number  | yes      | Maximum number of results (> 0)                                                                                                                                                                                                                                                                        |
| `filter`       | object  | no       | Metadata filter. See [Metadata filter](#metadata-filter) below.                                                                                                                                                                                                                                        |
| `namespace`    | string  | no       | Restricts results to this namespace (multitenancy).                                                                                                                                                                                                                                                    |
| `vector_field` | string  | no       | Vector field to search against: omitted or `"default"` = main vector; another name (e.g. `"title_vector"`, `"content_vector"`) = named index. Returns 400 if field does not exist on any point.                                                                                                        |
| `budget_ms`    | number  | no       | Maximum budget in ms. If the estimate exceeds it, returns 422 without executing the search.                                                                                                                                                                                                            |
| `rerank`       | boolean | no       | When `true`, re-scores candidates with Cross-Encoder (ONNX) and returns the top `limit` reordered. Requires the server to have a re-ranking model configured and the collection dimension to match the model's. If no reranker or incompatible dimension, search is performed without re-ranking.       |

**Request schema:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": null,
  "vector_field": "content_vector",
  "budget_ms": 50,
  "rerank": false
}
```

**Response:** `200 OK`

**Response schema:** each result may include `namespace` when the point has a namespace. The `rerank_ms` field is present only when `rerank: true` was sent and the server applied re-ranking (model loaded and compatible dimension).

```json
{
  "results": [
    {
      "id": "doc-1",
      "score": 0.92,
      "metadata": { "text": "Content" },
      "namespace": "tenant-a"
    }
  ],
  "took_ms": 2,
  "query_id": "optional-uuid",
  "rerank_ms": 12
}
```

| Response field | Type   | Description                                                                                                      |
| -------------- | ------ | ---------------------------------------------------------------------------------------------------------------- |
| `results`      | array  | List of points ordered by similarity (or by Cross-Encoder score when rerank was applied).                        |
| `took_ms`      | number | Total search time (ms).                                                                                          |
| `query_id`     | string | (optional) Query ID for debug (`GET /api/v1/debug/query-profile/{query_id}`).                                    |
| `rerank_ms`    | number | (optional) Time spent on re-ranking (ms). Present only when `rerank: true` and the server applied re-ranking.    |

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search \
  -H "Content-Type: application/json" \
  -d '{"vector":[0.1,0.2,-0.1],"limit":5,"namespace":"tenant-a"}'
```

#### Metadata filter

When the `filter` field is provided, the search uses **native HNSW pre-filtering**: the filter is applied during graph exploration (nodes that do not satisfy the filter are ignored before entering the candidate list). Filtered searches **guarantee** returning up to `limit` results that satisfy the filter: exploration continues (with progressive increase of the search parameter) until that number of valid results is reached or the graph is exhausted, without relying on a fixed multiplier.

The `filter` field is a JSON object. Each key is a metadata field; the value can be:

- **Direct value** — treated as equality (`$eq`): `{"source": "manual"}`.
- **Object with operators** — use `$eq`, `$ne`, `$in`, `$gt`, `$lt`, `$gte`, `$lte`:
  - `$eq`, `$ne`: exact value (any JSON type).
  - `$in`: array of allowed values.
  - `$gt`, `$lt`, `$gte`, `$lte`: numeric comparison (the metadata field must be a number).
- **Reserved key `$namespace`** — restricts results to the indicated namespace (multitenancy): `{"$namespace": "tenant-id"}`. Can be combined with other conditions.

Alternatively, use the top-level `namespace` parameter in the search body instead of `$namespace` in the filter.

Multiple fields are combined with **AND**. Example:

```json
{
  "vector": [0.1, 0.2],
  "limit": 10,
  "filter": {
    "category": "tech",
    "price": { "$gte": 10, "$lte": 100 },
    "status": { "$in": ["active", "pending"] }
  }
}
```

---

### POST /api/v1/collections/{name}/search/hybrid

Hybrid search: combines vector and BM25 (keyword) results via a configurable fusion strategy. The collection must have been created with `enable_bm25: true`.

**Path:** `name` — collection name.

**Request body:**

| Field          | Type   | Required | Description                                                                                                           |
| -------------- | ------ | -------- | --------------------------------------------------------------------------------------------------------------------- |
| `query_text`   | string | yes      | Text for keyword search (BM25)                                                                                        |
| `query_vector` | array  | yes      | Vector for vector search                                                                                              |
| `limit`        | number | yes      | Maximum number of results (> 0)                                                                                       |
| `alpha`        | number | no       | Vector search weight 0..1 (default: 0.5). (1 - alpha) = keyword weight. Used with `fusion: "weighted"`               |
| `fusion`       | string | no       | Fusion strategy: `"weighted"` (default) or `"rrf"`                                                                    |
| `rrf_k`        | number | no       | k constant for RRF (default: 60). Only used when `fusion: "rrf"`. Larger values smooth rank differences               |
| `namespace`    | string | no       | Restricts results to this namespace (multitenancy)                                                                    |

**Fusion strategies:**

- **`weighted`** (default): Weights rankings by `alpha`. Score = `alpha × 1/(k + rank_vec) + (1-alpha) × 1/(k + rank_bm25)`. Allows controlling the balance between vector and keyword search.
- **`rrf`** (Reciprocal Rank Fusion): Pure rank-based fusion without weighting. Score = `Σ 1/(k + rank_i)` for each ranker. Produces more stable results as it treats all rankers equally and does not depend on the scale of the original scores.

**When to use RRF vs Weighted:**

- Use **weighted** when you want to manually control the balance between vector and keyword via `alpha`.
- Use **rrf** when you want more stable results, independent of each ranker's score scale.

**Request schema (weighted — default):**

```json
{
  "query_text": "how to deploy",
  "query_vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "alpha": 0.5
}
```

**Request schema (RRF):**

```json
{
  "query_text": "how to deploy",
  "query_vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "fusion": "rrf",
  "rrf_k": 60
}
```

**Response:** `200 OK` (same response schema as search)

```json
{
  "results": [{ "id": "doc-1", "score": 0.85, "metadata": { "text": "..." } }],
  "took_ms": 3
}
```

**curl example (weighted):**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/hybrid \
  -H "Content-Type: application/json" \
  -d '{"query_text":"deploy","query_vector":[0.1,0.2,-0.1],"limit":5,"alpha":0.5}'
```

**curl example (RRF):**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/hybrid \
  -H "Content-Type: application/json" \
  -d '{"query_text":"deploy","query_vector":[0.1,0.2,-0.1],"limit":5,"fusion":"rrf","rrf_k":60}'
```

---

### POST /api/v1/collections/{name}/search/explain

Vector search with detailed explanation of each result. Returns **why** each result was returned (or filtered): score breakdown, per-condition filter evaluation, ranking position and HNSW index statistics.

**Path:** `name` — collection name.

**Request body:**

| Field       | Type   | Required | Description                                                              |
| ----------- | ------ | -------- | ------------------------------------------------------------------------ |
| `vector`    | array  | yes      | Query vector (same dimension as the collection)                          |
| `limit`     | number | yes      | Maximum number of results (> 0)                                          |
| `filter`    | object | no       | Metadata filter. See [Metadata filter](#metadata-filter) above.          |
| `namespace` | string | no       | Restricts results to this namespace (multitenancy)                       |

**Request schema:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": { "category": "tech" },
  "namespace": "tenant-a"
}
```

**Response:** `200 OK`

**Response schema:**

```json
{
  "query_vector_norm": 0.245,
  "distance_metric": "Cosine",
  "candidates_scanned": 30,
  "candidates_after_filter": 5,
  "results": [
    {
      "id": "doc-1",
      "score": 0.12,
      "distance_metric": "Cosine",
      "raw_distance": 0.12,
      "score_breakdown": {
        "vector_score": 0.12
      },
      "filter_evaluation": {
        "conditions": [
          {
            "field": "category",
            "operator": "$eq",
            "expected": "tech",
            "actual": "tech",
            "passed": true
          }
        ],
        "passed": true
      },
      "rank_before_filter": 1,
      "rank_after_filter": 1
    }
  ],
  "index_stats": {
    "total_points": 1000,
    "hnsw_layers": 16,
    "ef_search_used": 50,
    "tombstones_skipped": 0
  },
  "explain_meta": {
    "candidates_visited": 30,
    "layers_traversed": 16,
    "tombstones_skipped": 0
  }
}
```

| Field                             | Type   | Description                                                                            |
| --------------------------------- | ------ | -------------------------------------------------------------------------------------- |
| `query_vector_norm`               | number | L2 norm of the query vector                                                            |
| `distance_metric`                 | string | Metric: `Cosine`, `Euclidean`, `DotProduct`                                            |
| `candidates_scanned`              | number | Total candidates scanned by the index                                                  |
| `candidates_after_filter`         | number | Candidates that passed the metadata filter (pre-filtering impact)                      |
| `results`                         | array  | Individually explained results                                                         |
| `results[].score_breakdown`       | object | Score components (`vector_score`, etc.)                                                |
| `results[].filter_evaluation`     | object | Detailed filter evaluation (present if filter was applied)                             |
| `results[].rank_before_filter`    | number | Ranking position before filters (1-indexed)                                            |
| `results[].rank_after_filter`     | number | Position after filters (1-indexed, 0 if did not pass)                                 |
| `index_stats`                     | object | HNSW index statistics at the time of search                                            |
| `explain_meta`                    | object | _Optional._ Metadata of the search traversal (HNSW). Absent if index does not provide.|
| `explain_meta.candidates_visited` | number | Number of distance comparisons performed during search                                 |
| `explain_meta.layers_traversed`   | number | Number of HNSW graph layers traversed                                                  |
| `explain_meta.tombstones_skipped` | number | Tombstones (removed points) skipped during search                                      |

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/explain \
  -H "Content-Type: application/json" \
  -d '{"vector":[0.1,0.2,-0.1],"limit":5,"filter":{"category":"tech"}}'
```

---

### POST /api/v1/collections/{name}/search/estimate

Estimates the cost of a vector search **before executing it**. Returns estimated latency, memory consumption, HNSW nodes that will be visited, whether the query is "expensive" and optimization recommendations. Does not execute any real search.

**Path:** `name` — collection name.

**Request body:**

| Field             | Type    | Required | Description                                                         |
| ----------------- | ------- | -------- | ------------------------------------------------------------------- |
| `limit`           | number  | yes      | Number of results that will be requested in the search              |
| `filter`          | object  | no       | Metadata filter (same format as the search endpoint)                |
| `namespace`       | string  | no       | Restricts the estimate to the scenario with this namespace filter   |
| `include_history` | boolean | no       | If `true`, includes historical latency data (p50/p95/p99/avg)       |

**Request schema:**

```json
{
  "limit": 10,
  "filter": { "category": "tech" },
  "include_history": true
}
```

**Response:** `200 OK`

**Response schema:**

```json
{
  "estimated_ms": 2.35,
  "confidence_range": [1.17, 8.0],
  "estimated_memory_bytes": 45320,
  "estimated_nodes_visited": 575,
  "is_expensive": false,
  "recommendations": [],
  "breakdown": {
    "index_scan_cost": 1.76,
    "filter_cost": 0.0003,
    "hydration_cost": 0.01,
    "network_overhead": 0.1
  },
  "historical_latency": {
    "p50_ms": 2.0,
    "p95_ms": 8.0,
    "p99_ms": 15.0,
    "avg_ms": 3.2,
    "total_queries": 1520
  }
}
```

| Field                     | Type    | Description                                                                    |
| ------------------------- | ------- | ------------------------------------------------------------------------------ |
| `estimated_ms`            | number  | Estimated latency in milliseconds                                              |
| `confidence_range`        | array   | Confidence range `[min, max]` in ms                                            |
| `estimated_memory_bytes`  | number  | Estimated bytes of memory the query will consume                               |
| `estimated_nodes_visited` | number  | Estimated HNSW nodes that will be visited                                      |
| `is_expensive`            | boolean | `true` if the estimate exceeds the historical p95                              |
| `recommendations`         | array   | Optimization suggestions (e.g. "reduce limit")                                 |
| `breakdown`               | object  | Individual cost components (index_scan, filter, hydration, network)            |
| `historical_latency`      | object  | Present if `include_history=true`. Percentiles and average historical latency  |

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/estimate \
  -H "Content-Type: application/json" \
  -d '{"limit":10,"filter":{"category":"tech"},"include_history":true}'
```

#### Budget-Based Queries

The `budget_ms` field in the `POST /search` endpoint allows defining a maximum latency budget. If the cost estimate exceeds the budget, the search **is not executed** and the server returns `422 Unprocessable Entity` with the detailed estimate:

```json
{
  "error": "budget_exceeded",
  "message": "estimated cost (12.5ms) exceeds budget (5ms)",
  "code": 422,
  "estimate": {
    "estimated_ms": 12.5,
    "confidence_range": [6.25, 18.75],
    "is_expensive": true,
    "recommendations": ["Consider reducing limit for better performance"],
    "breakdown": { "...": "..." }
  }
}
```

---

## Dashboard

### GET /dashboard

Returns the dashboard HTML page (single-file with Alpine.js, Tailwind CDN and Chart.js). Displays collection list with stats, queries/min chart (24h), top 10 slowest queries (`GET /api/v1/stats/slow-queries`) and latency distribution. Updates via polling every 5s using `GET /api/v1/stats/global`, `GET /api/v1/stats/queries`, `GET /api/v1/stats/slow-queries` and `GET /api/v1/collections/{name}/stats`.

**Response:** `200 OK` (HTML)

---

## Statistics and Analytics

Analytics endpoints read the `queries.log` file (JSONL) and maintain an in-memory cache for 1h.

**Background auto-reindex:** An internal worker iterates over all collections every 30 minutes and, when the tombstone ratio (`tombstone_count / total_indexed`) exceeds 20%, triggers an automatic reindex (same index swap logic as the reindex endpoints). The worker state is not exposed in any stats endpoint; observability is done via structured logs (`tracing`): start and end of each worker cycle and start and end of each compaction.

**Retention policy (data retention):** A background worker every 1 hour iterates over collections that have `retention_days` configured and compacts the `wal.log` file, removing entries with timestamps older than `(now - retention_days days)`. This reduces the WAL size and limits the history available for PITR to the configured period. Retention can be set at collection creation (`POST /api/v1/collections` with `retention_days`), changed via `PATCH /api/v1/collections/{name}` (body: `{ "retention_days": number | null }`) and configured on the Dashboard's **Settings** page ("Data retention" section).

### GET /api/v1/stats/global

Returns global statistics: server totals (collections, points) and aggregates for the last 24h from the query log.

**Response:** `200 OK`

**Response schema:**

```json
{
  "total_collections": 2,
  "total_points": 150,
  "total_queries_24h": 420,
  "avg_latency_ms": 3.5,
  "queries_per_minute": [{ "timestamp": 1738742400, "count": 12 }],
  "simd_enabled": true,
  "role": "leader",
  "namespace_physical_isolation": false,
  "hnsw_auto_tune_enabled": true,
  "index_optimization_label": "Optimized by FerresEngine"
}
```

| Field                          | Type    | Description                                                                              |
| ------------------------------ | ------- | ---------------------------------------------------------------------------------------- |
| `total_collections`            | number  | Number of collections                                                                    |
| `total_points`                 | number  | Sum of points across all collections                                                     |
| `total_queries_24h`            | number  | Queries in the last 24h (from log)                                                       |
| `avg_latency_ms`               | number  | Average latency (ms) in the last 24h                                                     |
| `queries_per_minute`           | array   | Per-minute buckets: `timestamp` (Unix of minute), `count`                                |
| `simd_enabled`                 | boolean | Whether SIMD instructions (AVX2/SSE4.1) are active at runtime in distance kernels        |
| `role`                         | string  | Replication role: `"leader"` or `"replica"` (experimental)                               |
| `namespace_physical_isolation` | boolean | Whether namespace storage is physically isolated (multitenancy)                          |
| `hnsw_auto_tune_enabled`       | boolean | Whether dynamic `ef_search` HNSW auto-tuning is active (FerresEngine)                   |
| `index_optimization_label`     | string  | Label for the dashboard, e.g. `"Optimized by FerresEngine"`                              |

**HNSW Auto-Tune (FerresEngine):** The server dynamically adjusts `ef_search` based on the observed P95 latency per collection (source: `query_stats`). Every 60 seconds, for each collection: if P95 < 10 ms and recall is the priority, `ef_search` is increased (up to a maximum); if P95 > 50 ms (proxy for CPU under stress), it is reduced. The logic is in `ferres_db_core::collection::Collection::apply_hnsw_auto_tune`. Current values are exposed in `GET /api/v1/collections/{name}/stats` (`ef_search_current`) and in the Dashboard (Overview: "Optimized by FerresEngine").

### GET /api/v1/cluster

Returns the cluster state (active nodes, leader and replication status). Used by the Dashboard's **Cluster** page to show Leader and Followers. In standalone mode (without `raft` feature), returns a single node with role `leader` or `replica` depending on `--replica-of`. With the `raft` feature active, reflects the Raft state (nodes, leader_id).

**Response:** `200 OK`

**Response schema:**

```json
{
  "raft_enabled": false,
  "leader_id": "1",
  "nodes": [
    {
      "id": "1",
      "addr": "127.0.0.1:8080",
      "role": "leader",
      "replication_lag": null
    }
  ]
}
```

| Field                     | Type    | Description                                                                              |
| ------------------------- | ------- | ---------------------------------------------------------------------------------------- |
| `raft_enabled`            | boolean | Whether Raft consensus is active (built with `--features raft` and configured).          |
| `leader_id`               | string  | _Optional._ Leader node ID (e.g. `"1"`). Absent if no leader yet.                       |
| `nodes`                   | array   | List of known nodes (including this one).                                                |
| `nodes[].id`              | string  | Node identifier.                                                                         |
| `nodes[].addr`            | string  | Address (host:port).                                                                     |
| `nodes[].role`            | string  | Role: `"leader"`, `"follower"`, `"learner"` or `"replica"` (replica mode).              |
| `nodes[].replication_lag` | number  | _Optional._ Replication lag (index of last log applied). For followers only.            |

**curl example:**

```bash
curl -s -H "Authorization: Bearer YOUR_KEY" http://localhost:8080/api/v1/cluster
```

---

## Replication (Experimental)

Read Replicas allow scaling reads (search, listing) while maintaining a single leader node for writes. This feature is **experimental**.

### Replica mode

- **Initialization:** Start the server with `--replica-of <ADDR>` or set the environment variable `FERRESDB_REPLICA_OF` (e.g. `127.0.0.1:50051` for the leader's gRPC).
- **Behavior:** The node starts as a **replica**: accepts only read operations (GET, and POST on `/search`, `/search/hybrid`, `/search/explain`, `/search/estimate`, `/auth/login`). Any other write (POST/PUT/DELETE on collections, points, save, reindex, etc.) returns **405 Method Not Allowed** with body `{ "error": "method_not_allowed", "message": "Write operations are not allowed on a read replica", "code": 405 }`.
- **Replication worker:** With the `grpc` feature active, a background worker connects to the leader via gRPC, lists the collections, and for each one consumes the WAL via RPC `StreamWal(collection_name, from_position)`, applying upserts and deletes to the local VectorDB. The leader exposes `StreamWal` in the FerresDB service (proto `ferresdb.v1`).
- **API and Dashboard:** `GET /api/v1/stats/global` includes the `role` field (`"leader"` or `"replica"`). The Dashboard (Overview) shows a visual indicator "Role: Leader" or "Role: Replica".

### Incremental WAL (core)

In core, the method `Wal::stream_from(collection_dir, position)` returns WAL entries starting from index `position` (0-based), allowing the leader to serve only new entries in subsequent `StreamWal` calls.

### Requirements

- Leader and replica must use the server build with **`grpc` feature** for the replication worker and `StreamWal` RPC.
- The address in `--replica-of` must be the host:port of the leader's **gRPC server** (default port 50051, configurable with `GRPC_PORT`).

---

### GET /api/v1/stats/analytics

Returns consolidated JSON for the dashboard: tier distribution, latency (avg, P50/P95/P99, per-minute history in 24h), tombstones, circuit breaker, **time series for the last 10 minutes** and **search_cache Cache Hit Rate**.

**Response:** `200 OK`

**Time series aggregation fields (last 10 min):**

| Field                                   | Type           | Description                                                                                                                                                                                                                       |
| --------------------------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `time_series_10m`                       | object         | Aggregates of the 10-minute window for real-time monitoring                                                                                                                                                                       |
| `time_series_10m.avg_points_per_second` | number         | Average points inserted per second (ingestion) in the last 10 min                                                                                                                                                                |
| `time_series_10m.p95_latency_ms`        | number         | P95 search latency (ms) in the last 10 min (queries.log)                                                                                                                                                                         |
| `time_series_10m.throughput_per_minute` | array          | Per-minute buckets for throughput chart: `{ "timestamp": number, "points": number }`                                                                                                                                             |
| `time_series_10m.recent_latencies`      | array          | Recent queries for latency chart: `{ "timestamp": number, "took_ms": number }` (up to 100 entries)                                                                                                                               |
| `cache_hit_rate_pct`                    | number \| null | search_cache (core) hit rate percentage aggregated across all collections; `null` if no searches yet                                                                                                                              |
| `top_namespaces_by_storage`             | array          | Top namespaces by storage (up to 30): `{ "namespace": string, "point_count": number, "storage_bytes_estimate": number }`. Identifies tenants consuming the most resources. Points without namespace appear as `"(default)"`.      |
| `rerank_overhead_ms_avg`                | number \| null | Average time spent on re-ranking (ms) in recent queries that used rerank; `null` if none. Used in Dashboard (Analytics) as the "Re-ranking Overhead (ms)" metric.                                                                 |

The ingestion buffer is fed on every upsert (REST, gRPC and WebSocket). For the **10-minute** window, the analytics endpoint uses fresh reading of `queries.log` (without relying on the 1h cache), so that `time_series_10m.throughput_per_minute`, `time_series_10m.recent_latencies` and `time_series_10m.p95_latency_ms` reflect the most recent data. Cache Hit Rate is calculated from the `search_cache_hits` and `search_cache_misses` counters per collection (when `search_cache_size` > 0).

**SIMD flag:** The `simd_enabled` field is not in the body of `GET /api/v1/stats/analytics`; use `GET /api/v1/stats/global`, which returns `simd_enabled: boolean` indicating whether AVX2/SSE4.1 instructions are active in distance kernels.

---

### GET /api/v1/stats/queries

Lists queries from the last 24h (reads from `queries.log`, 1h cache).

**Query params:**

| Param        | Type   | Default | Description                                         |
| ------------ | ------ | ------- | --------------------------------------------------- |
| `collection` | string | —       | Filter by collection name                           |
| `limit`      | number | 100     | Maximum entries                                     |
| `sort`       | string | —       | `latency` = sort by latency (slowest first)         |

**Response:** `200 OK`

**Response schema:** array of objects:

```json
[
  {
    "timestamp": "2025-02-05T12:00:00Z",
    "collection": "docs",
    "limit": 10,
    "filter": null,
    "took_ms": 5,
    "results_count": 10,
    "query_id": "uuid"
  }
]
```

---

### GET /api/v1/stats/slow-queries

Queries with latency above the threshold (reads from `queries.log`, 1h cache).

**Query params:**

| Param          | Type   | Default | Description             |
| -------------- | ------ | ------- | ----------------------- |
| `threshold_ms` | number | 100     | Minimum latency (ms)    |
| `limit`        | number | 10      | Maximum entries         |

**Response:** `200 OK` — same array schema as `GET /api/v1/stats/queries`.

---

### GET /api/v1/collections/{name}/stats

Returns usage statistics for the collection (points, queries, latency percentiles and HNSW index parameters).

**Path:** `name` — collection name.

**Response:** `200 OK`

**Response schema:**

```json
{
  "num_points": 42,
  "num_queries": 100,
  "avg_latency_ms": 2.5,
  "p50_latency_ms": 2.0,
  "p95_latency_ms": 5.0,
  "p99_latency_ms": 8.0,
  "tombstone_count": 0,
  "tombstone_memory_waste_bytes": 0,
  "ef_search_current": 50,
  "hnsw_auto_tune_enabled": true
}
```

| Field                                                                  | Type    | Description                                                                    |
| ---------------------------------------------------------------------- | ------- | ------------------------------------------------------------------------------ |
| `num_points`                                                           | number  | Number of points in the collection                                             |
| `num_queries`                                                          | number  | Total registered queries (recent buffer)                                       |
| `avg_latency_ms`, `p50_latency_ms`, `p95_latency_ms`, `p99_latency_ms` | number  | Average latency and percentiles (ms)                                           |
| `tombstone_count`                                                      | number  | Points marked as removed (still in index until reindex)                        |
| `tombstone_memory_waste_bytes`                                         | number  | Estimated bytes of tombstones (quantized index)                                |
| `ef_search_current`                                                    | number  | Current `ef_search` HNSW value used in search (may be auto-tuned)             |
| `hnsw_auto_tune_enabled`                                               | boolean | Whether FerresEngine auto-tuning is active for this instance                  |

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/stats
```

---

## Debug (query profiling)

### GET /api/v1/debug/query-profile/{query_id}

Returns the execution profile of a query (time per phase: validation, search, hydrate). Useful for performance debugging when the search response includes `query_id`.

**Path:** `query_id` — UUID returned in `SearchPointsResponse.query_id`.

**Response:** `200 OK`

**Response schema:**

```json
{
  "query_id": "uuid",
  "total_ms": 12,
  "phases": [
    { "name": "validation", "duration_ms": 1, "percentage": 8.33 },
    { "name": "search", "duration_ms": 8, "percentage": 66.67 },
    { "name": "hydrate", "duration_ms": 3, "percentage": 25.0 }
  ]
}
```

| Field                  | Type   | Description                                  |
| ---------------------- | ------ | -------------------------------------------- |
| `query_id`             | string | Query ID                                     |
| `total_ms`             | number | Total time in ms                             |
| `phases`               | array  | Time per phase                               |
| `phases[].name`        | string | Phase name (validation, search, hydrate)     |
| `phases[].duration_ms` | number | Phase duration in ms                         |
| `phases[].percentage`  | number | Percentage of total                          |

**Error:** `404` with `error: "query_profile_not_found"` if the `query_id` does not exist (profiles are kept in memory with limited capacity).

**curl example:**

```bash
curl -s http://localhost:8080/api/v1/debug/query-profile/550e8400-e29b-41d4-a716-446655440000
```

---

## RBAC — Granular Access Control

FerresDB supports role-based access control (RBAC) with granular permissions per collection and metadata restrictions.

### Permissions Model

Each user has a `role` (Admin, Editor, Viewer) and optionally granular `permissions`. If `permissions` is configured, it takes precedence over the legacy role.

**Resources:**

| Type              | Description                          |
| ----------------- | ------------------------------------ |
| `all_collections` | Wildcard: all collections            |
| `collection`      | Specific collection (by name)        |

**Actions:**

| Action   | Description                         |
| -------- | ----------------------------------- |
| `read`   | search, get, list                   |
| `write`  | upsert, delete points               |
| `create` | create collection                   |
| `delete` | delete collection                   |
| `admin`  | manage users, save, etc.            |

**Metadata Restriction:**

Optional. When present, search results are automatically filtered (AND with request filters). Ensures data isolation per team/department.

**Namespace Allowance (API keys):**

API keys can be restricted to one or more namespaces (multitenancy). When `allowed_namespaces` is set on a key (non-empty list), that key may only access the requested namespace if it appears in the list. If the key has no restriction (null or empty), it may access any namespace.

- **Validation:** The server checks the requested namespace from: (1) query parameter `namespace` (e.g. `GET /api/v1/collections?namespace=tenant-a`), (2) header `X-Namespace`, and (3) request body fields `namespace` (search, upsert, delete points). If the key is restricted and the requested namespace is not in its list, the server returns `403` with `code: "forbidden_namespace"`.
- **Creating/updating keys:** Use `allowed_namespaces` in `POST /api/v1/keys` (optional) and `PUT /api/v1/keys/:id` (body: `{ "allowed_namespaces": ["tenant-a", "tenant-b"] }` or `null` for "all"). Omitted or empty list = no restriction (all namespaces).

### API Keys (list, create, update namespaces, delete)

**GET /api/v1/keys** — Lists all keys (Editor or Admin). Response: array of objects with `id`, `name`, `key_prefix`, `created_at`, and optionally `allowed_namespaces` (string array; absent or empty = all namespaces).

**POST /api/v1/keys** — Creates a new API key (Editor or Admin).

**Request body:**

| Field                | Type     | Required | Description                                                          |
| -------------------- | -------- | -------- | -------------------------------------------------------------------- |
| `name`               | string   | yes      | Key name (e.g. "production", "staging").                             |
| `allowed_namespaces` | string[] | no       | List of allowed namespaces. Omitted or empty = access to all.        |

**Response:** `200 OK` with `id`, `name`, `key`, `key_prefix`, `created_at`. The `key` value is shown only once.

**PUT /api/v1/keys/{id}** — Updates allowed namespaces (Editor or Admin).

**Request body:**

| Field                | Type            | Description                                      |
| -------------------- | --------------- | ------------------------------------------------ |
| `allowed_namespaces` | array or `null` | List of namespaces or `null` for "all".          |

**Response:** `200 OK` with `{ "updated": true, "id": <id> }`.

**DELETE /api/v1/keys/{id}** — Removes the key (Editor or Admin). Response: `200 OK` with `{ "deleted": true, "id": <id> }`.

### POST /api/v1/users (with permissions)

Creates a user with granular permissions.

**Request body (example with permissions):**

```json
{
  "username": "analyst",
  "password": "secret123",
  "role": "viewer",
  "permissions": [
    {
      "resource": { "type": "collection", "name": "sales-data" },
      "actions": ["read"],
      "metadata_restriction": {
        "field": "department",
        "allowed_values": ["sales"]
      }
    },
    {
      "resource": { "type": "collection", "name": "public-docs" },
      "actions": ["read", "write"]
    }
  ]
}
```

**curl example:**

```bash
curl -s -X POST http://localhost:8080/api/v1/users \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <admin-key>" \
  -d '{"username":"analyst","password":"secret","role":"viewer","permissions":[{"resource":{"type":"all_collections"},"actions":["read"]}]}'
```

---

### PUT /api/v1/users/{username}/permissions

Updates granular permissions for a user (Admin only).

**Path:** `username` — username.

**Request body:**

```json
{
  "permissions": [
    {
      "resource": { "type": "all_collections" },
      "actions": ["read"]
    }
  ]
}
```

To remove granular permissions (revert to legacy role behavior):

```json
{
  "permissions": null
}
```

**Response:** `200 OK`

```json
{
  "updated": true,
  "username": "analyst"
}
```

**curl example:**

```bash
curl -s -X PUT http://localhost:8080/api/v1/users/analyst/permissions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <admin-key>" \
  -d '{"permissions":[{"resource":{"type":"collection","name":"docs"},"actions":["read","write"]}]}'
```

---

### Permission Enforcement

| Endpoint                                 | Required Permission |
| ---------------------------------------- | ------------------- |
| POST /collections                        | `create`            |
| DELETE /collections/{name}               | `delete`            |
| POST /collections/{name}/points          | `write`             |
| POST /collections/{name}/points/link     | `write`             |
| DELETE /collections/{name}/points        | `write`             |
| POST /collections/{name}/search          | `read`              |
| POST /collections/{name}/search/hybrid   | `read`              |
| POST /collections/{name}/search/explain  | `read`              |
| POST /collections/{name}/search/estimate | `read`              |
| GET /api/v1/audit                        | Admin only          |

**Precedence rules:**

1. **Admin** → always allowed
2. **Granular permissions** → checked if configured
3. **Legacy role** → Editor can read/write/create; Viewer can read

**MetadataRestriction:** If the user has a `metadata_restriction` in their Read permission, the filter is automatically injected (AND with request filters). Example: a user with `department=sales` will only see results with `department=sales`.

**Namespace (API keys):** If the API key has `allowed_namespaces` set, the server validates the requested namespace (query `namespace`, header `X-Namespace` or body `namespace`). If not allowed, returns `403` with `code: "forbidden_namespace"`.

---

## Audit Trail

FerresDB records all actions in a persistent audit trail (JSONL files with daily rotation).

### GET /api/v1/audit

Queries filtered audit entries (Admin only).

**Query params:**

| Param      | Type   | Default | Description                                                |
| ---------- | ------ | ------- | ---------------------------------------------------------- |
| `user`     | string | —       | Filter by user_id                                          |
| `action`   | string | —       | Filter by action (e.g. "search", "upsert", "login")        |
| `resource` | string | —       | Filter by resource (substring, e.g. "collection:docs")     |
| `from`     | string | 7d ago  | Start date/time (RFC 3339)                                 |
| `to`       | string | now     | End date/time (RFC 3339)                                   |
| `limit`    | number | 100     | Maximum entries (max: 1000)                                |

**Response:** `200 OK`

```json
[
  {
    "timestamp": "2026-02-07T14:30:00Z",
    "user_id": "analyst",
    "action": "search",
    "resource": "collection:sales-data",
    "details": { "query_id": "...", "limit": 10, "results_count": 5 },
    "result": "success",
    "ip_address": "192.168.1.100",
    "duration_ms": 3
  },
  {
    "timestamp": "2026-02-07T14:29:00Z",
    "user_id": "viewer_user",
    "action": "upsert",
    "resource": "collection:docs",
    "details": { "denied": true },
    "result": "denied"
  }
]
```

| Field         | Type   | Description                                                                                          |
| ------------- | ------ | ---------------------------------------------------------------------------------------------------- |
| `timestamp`   | string | UTC date/time (RFC 3339)                                                                             |
| `user_id`     | string | User who performed the action                                                                        |
| `action`      | string | Action: search, upsert, delete_points, create_collection, delete_collection, login, create_user, etc.|
| `resource`    | string | Resource: collection:name, user:name, api_key:name, system:collections                              |
| `details`     | object | Summary details (without vectors)                                                                    |
| `result`      | string | success, denied, error                                                                               |
| `ip_address`  | string | Client IP (if available)                                                                             |
| `duration_ms` | number | Duration in ms (if available)                                                                        |

**Audited actions:**

- `login` (success and failure)
- `search`, `search_hybrid`
- `upsert`, `delete_points`
- `create_collection`, `delete_collection`
- `create_user`, `delete_user`, `update_password`, `update_permissions`
- `create_api_key`, `delete_api_key`
- `save`

**curl example:**

```bash
# All actions in the last 24h
curl -s http://localhost:8080/api/v1/audit?limit=50 \
  -H "Authorization: Bearer <admin-key>"

# Actions from a specific user
curl -s "http://localhost:8080/api/v1/audit?user=analyst&action=search" \
  -H "Authorization: Bearer <admin-key>"

# Actions in a time range
curl -s "http://localhost:8080/api/v1/audit?from=2026-02-07T00:00:00Z&to=2026-02-08T00:00:00Z" \
  -H "Authorization: Bearer <admin-key>"
```

### Storage

Audit logs are written to JSONL files in the data directory:

```
data/logs/audit-2026-02-07.jsonl
data/logs/audit-2026-02-08.jsonl
```

- Daily rotation (new file per day)
- Append-only (like WAL)
- Asynchronous writes (do not impact handler latency)

---

## WebSocket Streaming

FerresDB supports real-time point ingestion and event subscription via WebSocket. The protocol is JSON over WebSocket with typed messages.

### GET /api/v1/ws

Endpoint for HTTP → WebSocket upgrade.

**Authentication:** Required. Accepts API key in two ways:

- **Query param:** `?token=sk-xxx`
- **Header:** `Authorization: Bearer <key>`

**Limits:**

| Parameter                  | Value                                     |
| -------------------------- | ----------------------------------------- |
| Maximum connections        | 100 (configurable)                        |
| Maximum message size       | 10 MB                                     |
| Heartbeat (ping)           | every 30s                                 |
| Pong timeout               | 10s (if not received, connection is closed)|
| Inactivity timeout         | 5 minutes                                 |
| Batch debounce (upsert)    | 10ms                                      |

**JavaScript connection example:**

```javascript
const ws = new WebSocket("ws://localhost:8080/api/v1/ws?token=sk-xxx");
ws.onopen = () => console.log("Connected");
ws.onmessage = (event) => console.log(JSON.parse(event.data));
```

**Example with wscat:**

```bash
wscat -c "ws://localhost:8080/api/v1/ws?token=sk-xxx"
```

**Connection errors:**

| Status | Description                               |
| ------ | ----------------------------------------- |
| `401`  | Invalid or missing API key                |
| `503`  | Simultaneous connection limit reached     |

---

### Message Protocol

All messages are JSON objects with a `type` discriminator field.

#### Client → Server Messages

##### `upsert` — Real-time point ingestion

Inserts or updates points in a collection. Upsert messages are accumulated internally for 10ms (debounce) before flushing for better throughput at high frequency.

```json
{
  "type": "upsert",
  "collection": "my_collection",
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "metadata": { "text": "document content" }
    },
    {
      "id": "doc-2",
      "vector": [0.3, -0.1, 0.5],
      "metadata": { "text": "another document" }
    }
  ]
}
```

| Field        | Type   | Required | Description                              |
| ------------ | ------ | -------- | ---------------------------------------- |
| `type`       | string | yes      | Always `"upsert"`                        |
| `collection` | string | yes      | Target collection name                   |
| `points`     | array  | yes      | Array of points (id, vector, metadata)   |

Each point:

| Field      | Type   | Required | Description                                   |
| ---------- | ------ | -------- | --------------------------------------------- |
| `id`       | string | yes      | Unique point ID                               |
| `vector`   | array  | yes      | Float array (same dimension as collection)    |
| `metadata` | object | no       | Arbitrary JSON (default: `{}`)                |

**Response:** `ack` message (see below).

##### `subscribe` — Subscription to collection events

Subscribes the connection to receive real-time notifications when points are inserted or deleted in a collection.

```json
{
  "type": "subscribe",
  "collection": "my_collection",
  "events": ["upsert", "delete"]
}
```

| Field        | Type   | Required | Description                                                                                        |
| ------------ | ------ | -------- | -------------------------------------------------------------------------------------------------- |
| `type`       | string | yes      | Always `"subscribe"`                                                                               |
| `collection` | string | yes      | Collection name to subscribe to                                                                    |
| `events`     | array  | no       | Event type filter: `["upsert"]`, `["delete"]`, or both. If empty/absent, receives all.             |

**Response:** `ack` message confirming the subscription (with `upserted: 0, failed: 0, took_ms: 0`).

**Errors:**

- `404` — collection not found
- `409` — already subscribed to this collection

##### `ping` — Application heartbeat

```json
{
  "type": "ping"
}
```

**Response:** `pong` message.

---

#### Server → Client Messages

##### `ack` — Operation confirmation

Sent after a successful `upsert` or `subscribe`.

```json
{
  "type": "ack",
  "upserted": 10,
  "failed": 0,
  "took_ms": 5
}
```

| Field      | Type   | Description                                      |
| ---------- | ------ | ------------------------------------------------ |
| `type`     | string | Always `"ack"`                                   |
| `upserted` | number | Points successfully inserted/updated             |
| `failed`   | number | Points that failed (invalid dimension, etc.)     |
| `took_ms`  | number | Processing time in ms                            |

##### `event` — Collection change notification

Sent to subscribers when points are inserted or deleted (via REST or WebSocket).

```json
{
  "type": "event",
  "collection": "my_collection",
  "action": "upsert",
  "point_ids": ["doc-1", "doc-2"],
  "timestamp": 1707123456
}
```

| Field        | Type   | Description                                  |
| ------------ | ------ | -------------------------------------------- |
| `type`       | string | Always `"event"`                             |
| `collection` | string | Collection name                              |
| `action`     | string | Operation type: `"upsert"` or `"delete"`     |
| `point_ids`  | array  | IDs of affected points                       |
| `timestamp`  | number | UNIX timestamp (seconds) of the operation    |

**Note:** Events are emitted both by REST operations (`POST /points`, `DELETE /points`) and by WebSocket upserts. All active subscribers receive the notification.

##### `error` — Error

```json
{
  "type": "error",
  "message": "collection not found",
  "code": 404
}
```

| Field     | Type   | Description                                     |
| --------- | ------ | ----------------------------------------------- |
| `type`    | string | Always `"error"`                                |
| `message` | string | Human-readable error message                    |
| `code`    | number | Semantic HTTP code (400, 404, 408, 409, 500)    |

Common error codes:

| Code | Description                              |
| ---- | ---------------------------------------- |
| 400  | Invalid or malformed JSON message        |
| 404  | Collection not found                     |
| 408  | Timeout (inactivity or pong)             |
| 409  | Already subscribed to this collection    |
| 500  | Internal error (lock, insert, etc.)      |

##### `pong` — Ping response

```json
{
  "type": "pong"
}
```

---

### Batch Behavior (Debounce)

`upsert` messages sent in quick succession are automatically accumulated for **10ms** before being processed. This significantly improves throughput in high-frequency ingestion scenarios:

1. The client sends multiple `upsert` messages quickly
2. The server accumulates all of them during the 10ms window
3. Points are grouped by collection
4. Each group is inserted as a single batch
5. One `ack` is sent per collection with the consolidated total

---

### Heartbeat and Timeouts

The server maintains the connection with bidirectional heartbeat:

1. **Server ping** (every 30s): the server sends `{"type":"ping"}` to the client
2. **Client pong**: the client must respond with `{"type":"ping"}` (which receives `{"type":"pong"}`)
3. **Pong timeout**: if the pong does not arrive within a heartbeat interval (~30s), the connection is closed with `{"type":"error","message":"pong timeout","code":408}`
4. **Inactivity timeout**: if there is no activity (no messages received) for 5 minutes, the connection is closed with `{"type":"error","message":"inactivity timeout","code":408}`

---

### Prometheus Metrics

WebSocket exposes metrics for observability:

| Metric                       | Type    | Labels | Description                                          |
| ---------------------------- | ------- | ------ | ---------------------------------------------------- |
| `ws_connections_active`      | Gauge   | —      | Currently active WebSocket connections               |
| `ws_messages_received_total` | Counter | `type` | Messages received (text, upsert, subscribe, ping)    |
| `ws_messages_sent_total`     | Counter | `type` | Messages sent (outgoing, event)                      |

---

### Full Example: Ingestion + Subscription

```javascript
// Connect
const ws = new WebSocket("ws://localhost:8080/api/v1/ws?token=sk-xxx");

ws.onopen = () => {
  // 1. Subscribe to events from the "docs" collection
  ws.send(
    JSON.stringify({
      type: "subscribe",
      collection: "docs",
      events: ["upsert", "delete"],
    }),
  );

  // 2. Insert points via WebSocket
  ws.send(
    JSON.stringify({
      type: "upsert",
      collection: "docs",
      points: [
        {
          id: "ws-1",
          vector: [0.1, 0.2, 0.3],
          metadata: { text: "real-time data" },
        },
        {
          id: "ws-2",
          vector: [0.4, 0.5, 0.6],
          metadata: { text: "streaming insert" },
        },
      ],
    }),
  );
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  switch (msg.type) {
    case "ack":
      console.log(
        `Upserted: ${msg.upserted}, Failed: ${msg.failed}, Took: ${msg.took_ms}ms`,
      );
      break;
    case "event":
      console.log(
        `Event: ${msg.action} on ${msg.collection}, IDs: ${msg.point_ids}`,
      );
      break;
    case "pong":
      console.log("Pong received");
      break;
    case "error":
      console.error(`Error ${msg.code}: ${msg.message}`);
      break;
  }
};

// Heartbeat: respond to server pings
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === "ping") {
    ws.send(JSON.stringify({ type: "ping" }));
  }
};
```

**Python example (websockets):**

```python
import asyncio
import json
import websockets

async def main():
    uri = "ws://localhost:8080/api/v1/ws?token=sk-xxx"
    async with websockets.connect(uri) as ws:
        # Subscribe
        await ws.send(json.dumps({
            "type": "subscribe",
            "collection": "docs"
        }))

        # Upsert
        await ws.send(json.dumps({
            "type": "upsert",
            "collection": "docs",
            "points": [
                {"id": "py-1", "vector": [0.1, 0.2, 0.3], "metadata": {"source": "python"}}
            ]
        }))

        # Receive messages
        async for message in ws:
            msg = json.loads(message)
            print(f"[{msg['type']}] {msg}")

asyncio.run(main())
```

---

## gRPC API (feature `grpc`)

FerresDB offers a **native gRPC API** as an alternative to the REST API, ideal for server-to-server communication with high performance, bidirectional streaming and automatic client generation in any language.

### Activation

The gRPC API is optional and controlled by feature flag. To build with gRPC support:

```bash
cargo build -p ferres-db-server --features grpc
```

The REST server continues to work normally even without the `grpc` feature.

### Ports

| Protocol  | Default port | Configuration        |
| --------- | ------------ | -------------------- |
| REST/HTTP | 8080         | `PORT` env or config |
| gRPC      | 50051        | `GRPC_PORT` env      |

Both servers run simultaneously when the feature is enabled.

### Proto file

The definition file is at `crates/server/proto/ferresdb.proto` (package `ferresdb.v1`).

### `FerresDB` service

```protobuf
service FerresDB {
  // Collections
  rpc CreateCollection(CreateCollectionRequest) returns (CreateCollectionResponse);
  rpc GetCollection(GetCollectionRequest) returns (GetCollectionResponse);
  rpc ListCollections(ListCollectionsRequest) returns (ListCollectionsResponse);
  rpc DeleteCollection(DeleteCollectionRequest) returns (DeleteCollectionResponse);

  // Points
  rpc UpsertPoints(UpsertPointsRequest) returns (UpsertPointsResponse);
  rpc DeletePoints(DeletePointsRequest) returns (DeletePointsResponse);
  rpc GetPoint(GetPointRequest) returns (GetPointResponse);
  rpc ListPoints(ListPointsRequest) returns (ListPointsResponse);

  // Search
  rpc Search(SearchRequest) returns (SearchResponse);
  rpc HybridSearch(HybridSearchRequest) returns (SearchResponse);
  rpc ExplainSearch(ExplainSearchRequest) returns (ExplainSearchResponse);

  // Bidirectional streaming
  rpc StreamUpsert(stream UpsertPointsRequest) returns (stream UpsertPointsResponse);
  rpc StreamSearch(stream SearchRequest) returns (stream SearchResponse);
}
```

### REST → gRPC mapping

| REST Endpoint                                    | gRPC RPC                        |
| ------------------------------------------------ | ------------------------------- |
| `POST /api/v1/collections`                       | `CreateCollection`              |
| `GET  /api/v1/collections`                       | `ListCollections`               |
| `GET  /api/v1/collections/{name}`                | `GetCollection`                 |
| `DELETE /api/v1/collections/{name}`              | `DeleteCollection`              |
| `POST /api/v1/collections/{name}/points`         | `UpsertPoints`                  |
| `POST /api/v1/collections/{name}/points/link`    | —                               |
| `DELETE /api/v1/collections/{name}/points`       | `DeletePoints`                  |
| `GET  /api/v1/collections/{name}/points/{id}`    | `GetPoint`                      |
| `GET  /api/v1/collections/{name}/points`         | `ListPoints`                    |
| `POST /api/v1/collections/{name}/search`         | `Search`                        |
| `POST /api/v1/collections/{name}/search/hybrid`  | `HybridSearch`                  |
| `POST /api/v1/collections/{name}/search/explain` | `ExplainSearch`                 |
| WebSocket streaming                              | `StreamUpsert` / `StreamSearch` |

### Differences from the REST API

- **Metadata**: in gRPC, metadata is transmitted as a JSON string in the `metadata_json` field (instead of inline JSON object).
- **Filters**: filters are JSON strings in the `filter_json` field.
- **Distance Metric**: protobuf enum `DistanceMetric` (1=Cosine, 2=DotProduct, 3=Euclidean).
- **Authentication**: the gRPC API does not include API key authentication middleware (ideal for internal networks/service mesh). For exposed environments, use a proxy with mTLS.

### Examples with `grpcurl`

**Create collection:**

```bash
grpcurl -plaintext -d '{
  "name": "embeddings",
  "dimension": 384,
  "distance": 1
}' localhost:50051 ferresdb.v1.FerresDB/CreateCollection
```

**List collections:**

```bash
grpcurl -plaintext localhost:50051 ferresdb.v1.FerresDB/ListCollections
```

**Upsert points:**

```bash
grpcurl -plaintext -d '{
  "collection": "embeddings",
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, 0.3],
      "metadata_json": "{\"source\": \"grpc\"}"
    }
  ]
}' localhost:50051 ferresdb.v1.FerresDB/UpsertPoints
```

**Vector search:**

```bash
grpcurl -plaintext -d '{
  "collection": "embeddings",
  "vector": [0.1, 0.2, 0.3],
  "limit": 5
}' localhost:50051 ferresdb.v1.FerresDB/Search
```

**Hybrid search:**

```bash
grpcurl -plaintext -d '{
  "collection": "embeddings",
  "query_text": "machine learning",
  "query_vector": [0.1, 0.2, 0.3],
  "limit": 10,
  "alpha": 0.7,
  "fusion": "rrf",
  "rrf_k": 60
}' localhost:50051 ferresdb.v1.FerresDB/HybridSearch
```

### Generating gRPC clients

**Python:**

```bash
pip install grpcio-tools
python -m grpc_tools.protoc \
  -I crates/server/proto \
  --python_out=. \
  --grpc_python_out=. \
  crates/server/proto/ferresdb.proto
```

**TypeScript/Node.js:**

```bash
npx grpc_tools_node_protoc \
  --js_out=import_style=commonjs,binary:. \
  --grpc_out=grpc_js:. \
  --ts_out=. \
  -I crates/server/proto \
  crates/server/proto/ferresdb.proto
```

**Go:**

```bash
protoc --go_out=. --go-grpc_out=. \
  -I crates/server/proto \
  crates/server/proto/ferresdb.proto
```

---

## Model Context Protocol (MCP)

FerresDB can act as an **MCP server** (Model Context Protocol) via STDIO, allowing clients such as Claude Desktop to connect to the binary and use tools for vector search, upsert and statistics. The protocol uses **stdin** for input and **stdout** for output; server logs are redirected to **stderr** when MCP mode is active, to avoid corrupting MCP messages.

### Activation

- **Command line:** run the binary with the `--mcp` flag.
- **Environment variable:** `FERRESDB_ENABLE_MCP=true` or `FERRESDB_ENABLE_MCP=1`.

The REST server (and gRPC, if enabled) remains active in the same process. MCP mode requires the binary to have been compiled with the `mcp` feature:

```bash
cargo build -p ferres-db-server --features mcp
```

Example for use with Claude Desktop (stdio):

```bash
/path/to/ferres-db-server --mcp
```

### MCP Tools

Three tools are available when the MCP server is active.

#### `search_points`

Vector similarity search in a collection. Uses the core's **native pre-filtering**: when `filter` or `namespace` is provided, the filter is applied during HNSW graph exploration (not just post-search).

| Argument       | Type   | Required | Description                                                                   |
| -------------- | ------ | -------- | ----------------------------------------------------------------------------- |
| `collection`   | string | yes      | Collection name.                                                              |
| `vector`       | array  | yes      | Query vector (array of numbers).                                              |
| `limit`        | number | yes      | Maximum number of results (1 to 10000).                                       |
| `filter`       | object | no       | Metadata filter (JSON). E.g. `{"category": "tech"}`.                          |
| `namespace`    | string | no       | Restricts to a logical namespace (multitenancy).                              |
| `vector_field` | string | no       | Vector field (omitted or `"default"` = main vector; other = named).           |

**Response (success):** object with key `results`, array of objects `{ "id", "score", "metadata", "namespace" }`.

**Example arguments:**

```json
{
  "collection": "docs",
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": { "category": "blog" },
  "namespace": "tenant-a"
}
```

#### `upsert_points`

Inserts or updates points in a collection. The MCP channel has no authentication (trusted channel). Reuses the same validation and insertion logic as the REST API (dimension, batch, `Point::new`, `insert_batch`).

| Argument     | Type   | Required | Description         |
| ------------ | ------ | -------- | ------------------- |
| `collection` | string | yes      | Collection name.    |
| `points`     | array  | yes      | Array of points.    |

Each element of `points` must have:

| Field       | Type   | Required | Description               |
| ----------- | ------ | -------- | ------------------------- |
| `id`        | string | yes      | Point identifier.         |
| `vector`    | array  | yes      | Vector (array of numbers).|
| `metadata`  | object | no       | JSON metadata.            |
| `namespace` | string | no       | Logical namespace.        |
| `ttl`       | number | no       | TTL in seconds.           |

**Response (success):** object `{ "upserted": number, "failed": array }`, where `failed` contains items with `id` and `reason` in case of per-point error.

#### `get_stats`

Returns global or per-collection statistics.

| Argument     | Type   | Required | Description                                                                 |
| ------------ | ------ | -------- | --------------------------------------------------------------------------- |
| `collection` | string | no       | If omitted: global statistics. If provided: collection statistics.          |

**Response (global):** `total_collections`, `total_points`, `total_queries_24h`, `avg_latency_ms`, `queries_per_minute`, `simd_enabled`.

**Response (per collection):** `num_points`, `num_queries`, `avg_latency_ms`, `p50_latency_ms`, `p95_latency_ms`, `p99_latency_ms`, `tombstone_count`, `tombstone_memory_waste_bytes`.

Errors (collection not found, invalid dimension, etc.) are returned as error content in the tool result (`error` / `message` structure in JSON).
