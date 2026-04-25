# Usage Guide — SDK and HTTP Clients

This document describes the official **Rust SDK** and how to use the FerresDB REST API from **Python** and **TypeScript/JavaScript**. For the complete endpoint reference, see [api.md](api.md).

---

## Rust SDK (official)

The **ferres-db-sdk** crate (`crates/sdk-rust`) provides the HTTP client and exposes hybrid search in a type-safe way. Collection and point operations (create, list, upsert, vector search) are done via REST API; use `reqwest` directly or the [API reference](api.md) to build the calls.

### Dependency

In your project's `Cargo.toml`:

```toml
[dependencies]
ferres-db-sdk = { path = "../ferres-db-core/crates/sdk-rust" }
# or, if published: ferres-db-sdk = "0.1"
```

### Client and hybrid search

```rust
use ferres_db_sdk::{FerresDbClient, HybridSearchResponse, SearchResultItem};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FerresDbClient::new("http://localhost:8080");

    let query_text = "how to deploy";
    let query_vector: Vec<f32> = vec![0.1; 384]; // use the same embedding as ingestion

    let response: HybridSearchResponse = client
        .hybrid_search("docs", query_text, &query_vector, 5, 0.5)
        .await?;

    println!("took {} ms", response.took_ms);
    for item in &response.results {
        println!("{} score={:.4} metadata={}", item.id, item.score, item.metadata);
    }

    Ok(())
}
```

### Public types

| Type                   | Description                                          |
| ---------------------- | ---------------------------------------------------- |
| `FerresDbClient`       | HTTP client; `new(base_url)`, `hybrid_search(...)`   |
| `HybridSearchResponse` | `{ results: Vec<SearchResultItem>, took_ms: u64 }`   |
| `SearchResultItem`     | `{ id, score, metadata }`                            |
| `SdkError`             | Network, API (status + message) or decode errors     |

### Error handling

```rust
match client.hybrid_search("docs", "text", &vec![0.1; 384], 5, 0.5).await {
    Ok(res) => { /* res.results */ }
    Err(ferres_db_sdk::SdkError::Api { status, message }) => {
        eprintln!("API error {}: {}", status, message);
    }
    Err(e) => { /* request/decode error */ }
}
```

Operations not in the Rust SDK (create collection, upsert, vector search) should use the REST API with `reqwest` and the schemas in [api.md](api.md).

### Framework Integrations

The SDK exposes a **VectorStore** wrapper for integration with the RAG ecosystem (LangChain, LlamaIndex and tools that expect a vector storage interface).

- **Module:** `ferres_db_sdk::integrations`
- **Trait:** `VectorStore` — async methods:
  - `add_vectors(ids, vectors, metadatas)` — inserts or updates documents (vector + metadata).
  - `similarity_search(query_vector, k)` — returns the `k` most similar documents (without score).
  - `similarity_search_with_score(query_vector, k)` — returns `(document, score)`.
- **Implementation:** `FerresDbVectorStore` — uses a `FerresDbClient` and the collection name.
- **Helper:** `FerresDbVectorStore::ensure_collection(client, name, dimension, distance)` — creates the collection if it doesn't exist (useful for demos and scripts).

Minimal example (collection already exists):

```rust
use ferres_db_sdk::{FerresDbClient, integrations::{FerresDbVectorStore, VectorStore}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FerresDbClient::new("http://localhost:8080");
    let store = FerresDbVectorStore::ensure_collection(
        client,
        "my_rag",
        384,
        "Cosine",
    ).await?;

    store.add_vectors(
        &["id1".into()],
        &[vec![0.1; 384]],
        Some(&[serde_json::json!({"text": "Document content"})]),
    ).await?;

    let docs = store.similarity_search(&vec![0.1; 384], 5).await?;
    for d in docs {
        println!("{} {}", d.id, d.metadata);
    }
    Ok(())
}
```

Full example: `crates/sdk-rust/examples/langchain_integration.rs`. Run with the FerresDB server at `http://localhost:8080` (or `FERRESDB_URL`):

```bash
cargo run -p ferres-db-sdk --example langchain_integration
```

---

## Using the API from Python

There is no official Python SDK. Use the REST API with `requests` or `httpx`. The examples below follow the schemas from [api.md](api.md).

### Setup

```bash
pip install requests
```

```python
import requests

BASE = "http://localhost:8080"
session = requests.Session()
session.headers.setdefault("Content-Type", "application/json")
```

### Create collection

```python
def create_collection(
    name: str,
    dimension: int,
    distance: str = "Cosine",
    enable_bm25: bool = False,
    quantization: dict | None = None,
    retention_days: int | None = None,
):
    body = {
        "name": name,
        "dimension": dimension,
        "distance": distance,
        "enable_bm25": enable_bm25,
    }
    if quantization is not None:
        body["quantization"] = quantization
    if retention_days is not None:
        body["retention_days"] = retention_days
    resp = session.post(f"{BASE}/api/v1/collections", json=body)
    if resp.status_code not in (200, 201):
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json()

# SQ8 with QJL (residual correction, opt-in):
create_collection("docs", 384, quantization={
    "Scalar": {"dtype": "Int8", "always_ram": False, "quantile": 0.99,
               "enable_qjl": True, "qjl_m": 64, "qjl_seed": 42}
})

# PolarQuant (8 bits per angle):
create_collection("docs_polar", 384, quantization={"Polar": {"bits_per_angle": 8}})

# With 30-day retention:
create_collection("logs", 128, retention_days=30)
```

### Upsert points

Up to 1000 points per request. Vector dimension must match the collection's dimension.

```python
def upsert_points(collection: str, points: list[dict]):
    resp = session.post(
        f"{BASE}/api/v1/collections/{collection}/points",
        json={"points": points},
    )
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    data = resp.json()
    return data.get("upserted", 0), data.get("failed", [])

# Example: points with id, vector, metadata
points = [
    {"id": "doc-1", "vector": [0.1] * 384, "metadata": {"text": "Document content"}},
]
upserted, failed = upsert_points("docs", points)
```

### Vector search

```python
def search(
    collection: str,
    vector: list[float],
    limit: int = 5,
    filter_meta=None,
    rerank: bool = False,
):
    payload = {"vector": vector, "limit": limit}
    if filter_meta is not None:
        payload["filter"] = filter_meta
    if rerank:
        payload["rerank"] = True
    resp = session.post(f"{BASE}/api/v1/collections/{collection}/search", json=payload)
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    data = resp.json()
    # data["rerank_ms"] available when rerank=True was applied
    return data.get("results", [])
```

### Hybrid search (vector + BM25)

Requires a collection created with `enable_bm25: true`.

```python
def search_hybrid(
    collection: str,
    query_text: str,
    query_vector: list[float],
    limit: int = 5,
    alpha: float = 0.5,
    fusion: str = "weighted",
    rrf_k: int = 60,
):
    """
    fusion: "weighted" (default) or "rrf" (Reciprocal Rank Fusion).
    alpha: vector vs BM25 weight when fusion="weighted" (1.0 = vector only, 0.0 = BM25 only).
    rrf_k: k constant for RRF when fusion="rrf".
    """
    payload = {
        "query_text": query_text,
        "query_vector": query_vector,
        "limit": limit,
        "fusion": fusion,
    }
    if fusion == "weighted":
        payload["alpha"] = alpha
    else:
        payload["rrf_k"] = rrf_k
    resp = session.post(
        f"{BASE}/api/v1/collections/{collection}/search/hybrid",
        json=payload,
    )
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json().get("results", [])
```

### Graphs (relations between points)

```python
def link_points(collection: str, from_id: str, to_id: str):
    """Creates an undirected relation between two points."""
    resp = session.post(
        f"{BASE}/api/v1/collections/{collection}/points/link",
        json={"from": from_id, "to": to_id},
    )
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json()

def get_subgraph(
    collection: str,
    center_id: str | None = None,
    depth: int | None = None,
    seed: str | None = None,
    limit: int | None = None,
):
    """
    Returns subgraph via BFS.
    center_id + depth: expansion from a central node.
    seed: seed node for 1-hop.
    limit: maximum nodes in the full graph.
    Response: { "nodes": [...], "edges": [...] }
    """
    params = {}
    if center_id is not None:
        params["center_id"] = center_id
    if depth is not None:
        params["depth"] = depth
    if seed is not None:
        params["seed"] = seed
    if limit is not None:
        params["limit"] = limit
    resp = session.get(
        f"{BASE}/api/v1/collections/{collection}/graph/subgraph",
        params=params,
    )
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json()  # { "nodes": [...], "edges": [...] }

# Example:
link_points("docs", "doc-1", "doc-2")
subgraph = get_subgraph("docs", center_id="doc-1", depth=2)
```

### Repository reference

- [examples/simple_rag/app.py](../examples/simple_rag/app.py) — vector and hybrid search in the RAG pipeline.
- [examples/ingestion/ingest.py](../examples/ingestion/ingest.py) — collection creation and batch upsert.

### Best practices (Python)

- Reuse `requests.Session()` for connections and headers.
- Always check `resp.status_code` and the error body (`message` field when JSON).
- Upsert in batches of up to 1000 points.
- Use the **same dimension and same embedding provider** for ingestion and query.
- For RAG, include the `text` field (or the one configured in `bm25_text_field`) in the point metadata.

---

## Using the API from TypeScript/JavaScript

There is no official TypeScript SDK. Use `fetch` or `axios` with the same endpoints and schemas from [api.md](api.md).

### Setup (native fetch)

```typescript
const BASE = "http://localhost:8080";

async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...(options.headers as object),
    },
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error((err as { message?: string }).message || res.statusText);
  }
  return res.json();
}
```

### Create collection

```typescript
interface CreateCollectionBody {
  name: string;
  dimension: number;
  distance: "Cosine" | "Euclidean" | "DotProduct";
  enable_bm25?: boolean;
  bm25_text_field?: string;
}

await api("/api/v1/collections", {
  method: "POST",
  body: JSON.stringify({
    name: "docs",
    dimension: 384,
    distance: "Cosine",
    enable_bm25: true,
  } as CreateCollectionBody),
});
```

### Upsert points

```typescript
interface PointInput {
  id: string;
  vector: number[];
  metadata?: Record<string, unknown>;
}

const points: PointInput[] = [
  {
    id: "doc-1",
    vector: new Array(384).fill(0.1),
    metadata: { text: "Content" },
  },
];
const out = await api<{
  upserted: number;
  failed: { id: string; reason: string }[];
}>("/api/v1/collections/docs/points", {
  method: "POST",
  body: JSON.stringify({ points }),
});
console.log("upserted", out.upserted, "failed", out.failed.length);
```

### Vector search

```typescript
const results = await api<{
  results: { id: string; score: number; metadata: unknown }[];
  took_ms: number;
}>("/api/v1/collections/docs/search", {
  method: "POST",
  body: JSON.stringify({ vector: new Array(384).fill(0.1), limit: 5 }),
});
console.log(results.results, results.took_ms);
```

### Hybrid search

```typescript
const hybrid = await api<{
  results: { id: string; score: number; metadata: unknown }[];
  took_ms: number;
}>("/api/v1/collections/docs/search/hybrid", {
  method: "POST",
  body: JSON.stringify({
    query_text: "how to deploy",
    query_vector: new Array(384).fill(0.1),
    limit: 5,
    alpha: 0.5,
  }),
});
```

### Best practices (TypeScript/JavaScript)

- Use `async/await` and check `response.ok` before calling `response.json()`.
- Handle errors by parsing the error JSON and displaying `message`.
- Batch points (up to 1000 per request).
- Set timeouts (e.g. `AbortController` with `setTimeout` in `fetch`).

---

## General best practices

- **Timeouts:** Set a timeout on all clients (e.g. 30s for writes, 10s for searches).
- **Retries:** On 5xx errors or network failures, use retry with light backoff (e.g. 1s, 2s, 4s).
- **Logs:** Avoid logging full vectors; use only the dimension or a small preview.
- **Filters:** Use the `filter` parameter in vector search when you need to restrict by metadata (equality).
- **Hybrid search:** Prefer hybrid search when the collection has BM25 enabled; adjust `alpha` according to the desired weight (vector vs keyword).
