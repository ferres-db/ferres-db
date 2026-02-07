# Landing Page Brief — FerresDB (Lovable)

Use the text below as the full briefing to create a **FerresDB** landing page in Lovable.

---

## Design instructions (Lovable)

- **Goal:** A modern, clean landing page for FerresDB (high-performance vector database).
- **Color palette:**
  - **Accent / CTA / highlights / links:** `#f97316` (orange).
  - **Background and main text:** `#1A1A1A` (near black).
- **Style:** Modern typography, clean layout, good contrast. Emphasize CTAs and key numbers/benefits (e.g. latency in μs, throughput) with the accent color.

---

## What is FerresDB

**FerresDB** is a **high-performance** vector search engine written in **Rust**, designed for **semantic search**, **RAG** (Retrieval-Augmented Generation), and **recommendation systems**.

- It provides an **HTTP server** with a **REST API** to create collections, insert vectors (points), and search by similarity — **vector** and **hybrid with BM25** (text + vector).
- **Vector search** with HNSW index; metrics: Cosine, Euclidean, Dot Product.
- **Persistent** storage on disk (JSON-lines, WAL, crash recovery).
- Can be used as a **Rust library** or via the **server** in RAG pipelines.

---

## Differences and strengths vs a conventional vector database

| Aspect | FerresDB | Conventional vector database (generic) |
|--------|----------|----------------------------------------|
| **Performance** | Rust + HNSW; latency in microseconds (P50 ~100–500 μs); throughput of tens of thousands of points/second | Many in Python/Go; typically higher latency |
| **Persistence** | WAL (Write-Ahead Log), periodic snapshots (e.g. every 1000 ops), automatic crash recovery | Not all offer WAL + snapshots + recovery |
| **Search** | Vector (Cosine, Euclidean, Dot Product) + **hybrid search with BM25** (text + vector) | Often vector-only focus |
| **Operations** | Thread-safe, type-safe, API Keys, dashboard, Prometheus metrics, health check | Depends on the product |
| **Deploy** | Single stack (Docker/API), no dependency on proprietary cloud service | Many are managed-only or heavier |

**Other differentiators:**

- **Extensible:** `ANNIndex` trait allows swapping the search backend (e.g. other ANN indexes in the future).
- **Official SDKs:** TypeScript and Python (plus Rust).
- **Documentation:** REST API, SDK, architecture, and ADRs available in the repository.

---

## Performance and benchmarks

FerresDB is built for **low latency** and **high throughput**. Benchmarks use [Criterion.rs](https://github.com/bheisler/criterion.rs). Use these numbers on the landing page (with the accent color **#f97316** for key metrics).

**Reference hardware:** Modern CPU (Intel i7 / AMD Ryzen), 16 GB RAM. Results vary with hardware, HNSW configuration, and vector dimension.

### Indexing (throughput)

| Collection size | Points per second | Total time   |
|-----------------|-------------------|--------------|
| 1K vectors      | ~50K–100K         | ~10–20 ms    |
| 10K vectors     | ~30K–60K         | ~150–300 ms  |
| 100K vectors    | ~20K–40K         | ~2.5–5 s     |

You can index hundreds of thousands of vectors in seconds and keep ingesting at tens of thousands of points per second.

### Search (latency)

| Percentile | Latency      |
|-----------|---------------|
| **P50**   | ~100–500 μs   |
| **P95**   | ~200–1000 μs  |
| **P99**   | ~500–2000 μs  |

Search stays in the **microsecond** range even at scale, so RAG and semantic search stay fast for real-time applications.

### Why it’s fast

- **Rust:** No GC pauses; predictable, low-latency execution.
- **HNSW:** Approximate nearest-neighbor index optimized for fast search and good recall.
- **Thread-safe design:** Ready for multi-threaded servers and concurrent requests.
- **Optional LRU cache:** Configurable search cache to reduce repeated work.
- **Parallelization:** Large batches are parallelized (e.g. via Rayon) for higher throughput.

### Observability

- **Prometheus metrics:** `GET /metrics` for scraping (request counts, latencies, etc.).
- **Health check:** `GET /health` for load balancers and orchestration.
- **Query analytics:** API supports query profiling (e.g. `took_ms`, latency percentiles) for debugging and tuning.
- **Dashboard:** Web UI for collections, points, and basic stats.

Highlight on the landing: **sub-millisecond search**, **tens of thousands of points/second** ingestion, and **Rust + HNSW** as the technical foundation.

---

## Docker installation

### Option A — Docker Compose (recommended)

Build and start the backend (API) and frontend (dashboard) in one command:

```bash
docker-compose up -d
```

- **Backend (API):** port `8080` — `http://localhost:8080`
- **Frontend (dashboard):** port `3000` — `http://localhost:3000`
- **Persistence:** volume `ferres-data` (data kept across restarts)
- **Health check:** `GET http://localhost:8080/health`

**Main environment variables (optional):**

- `BACKEND_PORT` — backend port (default: 8080)
- `FRONTEND_PORT` — frontend port (default: 3000)
- `STORAGE_PATH` — data path in container (default: `/data`)
- `LOG_LEVEL` — log level (default: `info`)
- `VITE_API_BASE_URL` — backend URL used by the dashboard (default: `http://localhost:8080`)
- `VITE_API_KEY` — API key for the dashboard (optional)

**Stop services:**

```bash
docker-compose down
# Remove volumes: docker-compose down -v
```

### Option B — Official images (manual run)

**1. Pull images**

```bash
docker pull ferresdb/ferres-db-core
docker pull ferresdb/ferres-db-frontend
```

**2. Start the backend (API)**

```bash
docker run -d \
  --name ferres-db-core \
  -p 8080:8080 \
  -e PORT=8080 \
  -e STORAGE_PATH=/data \
  -e FERRESDB_API_KEYS=ferres_sk_your_key_here \
  -v ferres-data:/data \
  ferresdb/ferres-db-core
```

- **API:** http://localhost:8080

**3. Start the frontend (dashboard)**

With the backend already running:

```bash
docker run -d \
  --name ferres-db-frontend \
  -p 3000:80 \
  -e VITE_API_BASE_URL=http://localhost:8080 \
  -e VITE_API_KEY=ferres_sk_your_key_here \
  ferresdb/ferres-db-frontend
```

- **Dashboard:** http://localhost:3000

---

## SDK installation

### TypeScript / JavaScript

```bash
pnpm add @ferres-db/typescript-sdk
# or: npm install @ferres-db/typescript-sdk
# or: yarn add @ferres-db/typescript-sdk
```

**Minimal example:**

```typescript
import { VectorDBClient, DistanceMetric } from "@ferres-db/typescript-sdk";

const client = new VectorDBClient({
  baseUrl: "http://localhost:8080",
  apiKey: "ferres_sk_...",
});

const collection = await client.createCollection({
  name: "documents",
  dimension: 384,
  distance: DistanceMetric.Cosine,
});

await client.upsertPoints("documents", [
  { id: "doc-1", vector: [0.1, 0.2, /* ... */], metadata: { text: "Hello" } },
]);

const results = await client.search("documents", {
  vector: [0.1, 0.2, /* ... */],
  limit: 5,
});
```

**Full documentation:** [sdk/typescript/README.md](../sdk/typescript/README.md)

### Python

```bash
pip install ferres-db-python
```

Or from source:

```bash
cd sdk/python
pip install -e .
```

**Minimal example:**

```python
import asyncio
from vector_db_client import VectorDBClient, Point, DistanceMetric

async def main():
    async with VectorDBClient(
        base_url="http://localhost:8080",
        api_key="ferres_sk_...",
    ) as client:
        await client.create_collection(
            name="my-collection",
            dimension=128,
            distance=DistanceMetric.COSINE,
        )
        await client.upsert_points("my-collection", [
            Point(id="1", vector=[0.1, 0.2, 0.3], metadata={"text": "hello"}),
        ])
        results = await client.search(
            collection="my-collection",
            vector=[0.1, 0.2, 0.3],
            limit=10,
        )

asyncio.run(main())
```

**Full documentation:** [sdk/python/README.md](../sdk/python/README.md)

### Rust

In your `Cargo.toml`:

```toml
[dependencies]
ferres-db-sdk = { path = "../ferres-db-core/crates/sdk-rust" }
# or, if published: ferres-db-sdk = "0.1"
```

Use the REST API or `FerresDbClient` for hybrid search (text + vector). **Documentation:** [docs/sdk.md](sdk.md)

---

## Quick start (summary)

1. **Start the stack** — `docker-compose up -d` (or manual image runs as above).
2. **Create a collection** — via curl or SDK (e.g. `POST /api/v1/collections` with `name`, `dimension`, `distance`).
3. **Insert points and search** — `POST /api/v1/collections/{name}/points` (upsert) and `POST /api/v1/collections/{name}/search` (vector search).

Full API reference: [docs/api.md](api.md).

---

## Suggested CTAs and links for the landing page

- **Get started with Docker** — link to this Docker installation section or to the repository.
- **View documentation** — link to `docs/` or main README.
- **TypeScript SDK** — link to [sdk/typescript/README.md](../sdk/typescript/README.md).
- **Python SDK** — link to [sdk/python/README.md](../sdk/python/README.md).
- **Repository** — link to the FerresDB repository (GitHub or similar).
- **Dashboard** — after starting the frontend, link to `http://localhost:3000` (or demo URL if available).

Use **#f97316** for buttons and highlight links and **#1A1A1A** for background and main text.
