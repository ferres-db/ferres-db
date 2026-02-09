# Guia de uso — SDK e clientes HTTP

Este documento descreve o **SDK Rust** oficial e como usar a API REST do FerresDB a partir de **Python** e **TypeScript/JavaScript**. Para a referência completa dos endpoints, veja [api.md](api.md).

---

## SDK Rust (oficial)

O crate **ferres-db-sdk** (`crates/sdk-rust`) fornece o cliente HTTP e expõe busca híbrida de forma type-safe. Operações de coleções e pontos (criar, listar, upsert, busca vetorial) são feitas via API REST; use `reqwest` diretamente ou a [referência da API](api.md) para montar as chamadas.

### Dependência

No `Cargo.toml` do seu projeto:

```toml
[dependencies]
ferres-db-sdk = { path = "../ferres-db-core/crates/sdk-rust" }
# ou, se publicado: ferres-db-sdk = "0.1"
```

### Cliente e busca híbrida

```rust
use ferres_db_sdk::{FerresDbClient, HybridSearchResponse, SearchResultItem};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = FerresDbClient::new("http://localhost:8080");

    let query_text = "como fazer deploy";
    let query_vector: Vec<f32> = vec![0.1; 384]; // use o mesmo embedding da ingestão

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

### Tipos públicos

| Tipo                   | Descrição                                           |
| ---------------------- | --------------------------------------------------- |
| `FerresDbClient`       | Cliente HTTP; `new(base_url)`, `hybrid_search(...)` |
| `HybridSearchResponse` | `{ results: Vec<SearchResultItem>, took_ms: u64 }`  |
| `SearchResultItem`     | `{ id, score, metadata }`                           |
| `SdkError`             | Erros de rede, API (status + message) ou decode     |

### Tratamento de erros

```rust
match client.hybrid_search("docs", "text", &vec![0.1; 384], 5, 0.5).await {
    Ok(res) => { /* res.results */ }
    Err(ferres_db_sdk::SdkError::Api { status, message }) => {
        eprintln!("API error {}: {}", status, message);
    }
    Err(e) => { /* request/decode error */ }
}
```

Operações que não estão no SDK Rust (criar coleção, upsert, busca vetorial) devem usar a API REST com `reqwest` e os schemas em [api.md](api.md).

### Framework Integrations

O SDK expõe um wrapper **VectorStore** para integração com ecossistema RAG (LangChain, LlamaIndex e ferramentas que esperam uma interface de armazenamento vetorial).

- **Módulo:** `ferres_db_sdk::integrations`
- **Trait:** `VectorStore` — métodos assíncronos:
  - `add_vectors(ids, vectors, metadatas)` — insere ou atualiza documentos (vetor + metadata).
  - `similarity_search(query_vector, k)` — retorna os `k` documentos mais similares (sem score).
  - `similarity_search_with_score(query_vector, k)` — retorna `(documento, score)`.
- **Implementação:** `FerresDbVectorStore` — usa um `FerresDbClient` e o nome da coleção.
- **Helper:** `FerresDbVectorStore::ensure_collection(client, name, dimension, distance)` — cria a coleção se não existir (útil para demos e scripts).

Exemplo mínimo (coleção já existente):

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
        Some(&[serde_json::json!({"text": "Conteúdo do doc"})]),
    ).await?;

    let docs = store.similarity_search(&vec![0.1; 384], 5).await?;
    for d in docs {
        println!("{} {}", d.id, d.metadata);
    }
    Ok(())
}
```

Exemplo completo: `crates/sdk-rust/examples/langchain_integration.rs`. Execute com o servidor FerresDB em `http://localhost:8080` (ou `FERRESDB_URL`):

```bash
cargo run -p ferres-db-sdk --example langchain_integration
```

---

## Uso da API a partir de Python

Não há SDK oficial em Python. Use a API REST com `requests` ou `httpx`. Os exemplos abaixo seguem os schemas de [api.md](api.md).

### Configuração

```bash
pip install requests
```

```python
import requests

BASE = "http://localhost:8080"
session = requests.Session()
session.headers.setdefault("Content-Type", "application/json")
```

### Criar coleção

```python
def create_collection(name: str, dimension: int, distance: str = "Cosine", enable_bm25: bool = False):
    resp = session.post(
        f"{BASE}/api/v1/collections",
        json={
            "name": name,
            "dimension": dimension,
            "distance": distance,
            "enable_bm25": enable_bm25,
        },
    )
    if resp.status_code not in (200, 201):
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json()
```

### Upsert de pontos

Até 1000 pontos por request. Dimensão do vetor deve ser igual à da coleção.

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

# Exemplo: pontos com id, vector, metadata
points = [
    {"id": "doc-1", "vector": [0.1] * 384, "metadata": {"text": "Conteúdo do doc"}},
]
upserted, failed = upsert_points("docs", points)
```

### Busca vetorial

```python
def search(collection: str, vector: list[float], limit: int = 5, filter_meta=None):
    payload = {"vector": vector, "limit": limit}
    if filter_meta is not None:
        payload["filter"] = filter_meta
    resp = session.post(f"{BASE}/api/v1/collections/{collection}/search", json=payload)
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json().get("results", [])
```

### Busca híbrida (vetorial + BM25)

Requer coleção criada com `enable_bm25: true`.

```python
def search_hybrid(collection: str, query_text: str, query_vector: list[float], limit: int = 5, alpha: float = 0.5):
    resp = session.post(
        f"{BASE}/api/v1/collections/{collection}/search/hybrid",
        json={
            "query_text": query_text,
            "query_vector": query_vector,
            "limit": limit,
            "alpha": alpha,
        },
    )
    if resp.status_code != 200:
        err = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
        raise RuntimeError(err.get("message", resp.text))
    return resp.json().get("results", [])
```

### Referência no repositório

- [examples/simple_rag/app.py](../examples/simple_rag/app.py) — busca vetorial e híbrida no pipeline RAG.
- [examples/ingestion/ingest.py](../examples/ingestion/ingest.py) — criação de coleção e upsert em batch.

### Boas práticas (Python)

- Reutilize `requests.Session()` para conexões e headers.
- Trate sempre `resp.status_code` e o body de erro (campo `message` quando JSON).
- Faça upsert em batches de até 1000 pontos.
- Use a **mesma dimensão e mesmo provedor de embedding** na ingestão e na query.
- Para RAG, inclua o campo `text` (ou o configurado em `bm25_text_field`) no metadata dos pontos.

---

## Uso da API a partir de TypeScript/JavaScript

Não há SDK oficial em TypeScript. Use `fetch` ou `axios` com os mesmos endpoints e schemas de [api.md](api.md).

### Configuração (fetch nativo)

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

### Criar coleção

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

### Upsert de pontos

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
    metadata: { text: "Conteúdo" },
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

### Busca vetorial

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

### Busca híbrida

```typescript
const hybrid = await api<{
  results: { id: string; score: number; metadata: unknown }[];
  took_ms: number;
}>("/api/v1/collections/docs/search/hybrid", {
  method: "POST",
  body: JSON.stringify({
    query_text: "como fazer deploy",
    query_vector: new Array(384).fill(0.1),
    limit: 5,
    alpha: 0.5,
  }),
});
```

### Boas práticas (TypeScript/JavaScript)

- Use `async/await` e verifique `response.ok` antes de fazer `response.json()`.
- Trate erros parseando o JSON de erro e exibindo `message`.
- Faça batching de pontos (até 1000 por request).
- Defina timeouts (ex.: `AbortController` com `setTimeout` em `fetch`).

---

## Boas práticas gerais

- **Timeouts:** Configure timeout em todos os clientes (ex.: 30s para escrita, 10s para busca).
- **Retries:** Em erros 5xx ou falha de rede, use retry com backoff leve (ex.: 1s, 2s, 4s).
- **Logs:** Evite logar vetores completos; use apenas dimensão ou um preview pequeno.
- **Filtros:** Use o parâmetro `filter` na busca vetorial quando precisar restringir por metadata (igualdade).
- **Busca híbrida:** Prefira busca híbrida quando a coleção tiver BM25 habilitado; ajuste `alpha` conforme o peso desejado (vetorial vs keyword).
