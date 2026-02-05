# Referência da API HTTP — FerresDB

Referência dos endpoints REST do servidor FerresDB. Base URL de exemplo: `http://localhost:8080`.

## Convenções

- **Content-Type:** `application/json` para requests com body.
- **Erros:** Respostas de erro usam o schema abaixo e o status HTTP apropriado.

### Schema de erro

```json
{
  "error": "string (tipo do erro)",
  "message": "string (descrição)",
  "code": 400
}
```

| Campo    | Tipo   | Descrição                          |
| -------- | ------ | ---------------------------------- |
| `error`  | string | Tipo: `collection_not_found`, `collection_already_exists`, `invalid_payload`, `invalid_dimension`, `internal_error` |
| `message`| string | Mensagem legível                   |
| `code`   | number | Código HTTP (400, 404, 409, 500)   |

---

## Health

### GET /health

Verifica se o servidor está em execução.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "status": "OK"
}
```

**Exemplo curl:**

```bash
curl -s http://localhost:8080/health
```

---

## Métricas (Prometheus)

### GET /metrics

Retorna métricas no formato Prometheus (text/plain).

**Resposta:** `200 OK` (corpo em texto)

**Exemplo curl:**

```bash
curl -s http://localhost:8080/metrics
```

---

## Persistência

### POST /api/v1/save

Persiste todas as coleções no disco. Útil antes de reiniciar o servidor ou em testes de persistência.

**Request:** Sem body (ou `{}`).

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "ok": true
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/save
```

---

## Coleções

### POST /api/v1/collections

Cria uma nova coleção.

**Request body:**

| Campo             | Tipo    | Obrigatório | Descrição |
| ----------------- | ------- | ----------- | --------- |
| `name`            | string  | sim         | Nome único: apenas `a-zA-Z0-9_-` |
| `dimension`       | number  | sim         | Dimensão dos vetores (1–4096) |
| `distance`        | string  | sim         | Métrica: `Cosine`, `Euclidean`, `DotProduct` |
| `enable_bm25`     | boolean | não         | Habilita índice BM25 para busca híbrida (default: false) |
| `bm25_text_field` | string  | não         | Chave em metadata usada como texto para BM25 (default: `"text"`) |

**Schema de request:**

```json
{
  "name": "my_collection",
  "dimension": 384,
  "distance": "Cosine",
  "enable_bm25": false,
  "bm25_text_field": "text"
}
```

**Resposta:** `201 Created`

**Schema de resposta:**

```json
{
  "name": "my_collection",
  "dimension": 384,
  "distance": "Cosine",
  "created_at": 1707123456
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections \
  -H "Content-Type: application/json" \
  -d '{"name":"docs","dimension":384,"distance":"Cosine","enable_bm25":true}'
```

---

### GET /api/v1/collections

Lista todas as coleções.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "collections": [
    {
      "name": "docs",
      "dimension": 384,
      "num_points": 42,
      "created_at": 1707123456
    }
  ]
}
```

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/collections
```

---

### GET /api/v1/collections/{name}

Retorna detalhes de uma coleção.

**Path:** `name` — nome da coleção.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "name": "docs",
  "dimension": 384,
  "num_points": 42,
  "last_updated": 1707123456,
  "stats": {
    "index_size_bytes": 64512
  }
}
```

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs
```

---

### DELETE /api/v1/collections/{name}

Remove uma coleção e seus dados do disco.

**Path:** `name` — nome da coleção.

**Resposta:** `204 No Content` (sem body)

**Exemplo curl:**

```bash
curl -s -X DELETE http://localhost:8080/api/v1/collections/docs
```

---

## Pontos

### POST /api/v1/collections/{name}/points

Insere ou atualiza pontos em lote (até 1000 pontos por request).

**Path:** `name` — nome da coleção.

**Request body:**

| Campo   | Tipo  | Descrição |
| ------- | ----- | --------- |
| `points` | array | Lista de 1 a 1000 pontos (objeto abaixo). |

Cada elemento de `points`:

| Campo     | Tipo   | Obrigatório | Descrição |
| --------- | ------ | ----------- | --------- |
| `id`      | string | sim         | ID único do ponto |
| `vector`  | array  | sim         | Array de números (float), dimensão igual à da coleção |
| `metadata`| object | não         | JSON arbitrário (default: `{}`) |

**Schema de request:**

```json
{
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "metadata": { "text": "Conteúdo do documento" }
    }
  ]
}
```

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "upserted": 1,
  "failed": [
    { "id": "doc-2", "reason": "dimension mismatch: expected 384, got 128" }
  ]
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/points \
  -H "Content-Type: application/json" \
  -d '{"points":[{"id":"doc-1","vector":[0.1,0.2,-0.1],"metadata":{"text":"Hello"}}]}'
```

---

### DELETE /api/v1/collections/{name}/points

Remove pontos pelo ID.

**Path:** `name` — nome da coleção.

**Request body:**

```json
{
  "ids": ["doc-1", "doc-2"]
}
```

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "deleted": 2
}
```

**Exemplo curl:**

```bash
curl -s -X DELETE http://localhost:8080/api/v1/collections/docs/points \
  -H "Content-Type: application/json" \
  -d '{"ids":["doc-1","doc-2"]}'
```

---

### GET /api/v1/collections/{name}/points/{id}

Retorna um ponto pelo ID.

**Path:** `name` — nome da coleção; `id` — ID do ponto.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "id": "doc-1",
  "vector": [0.1, 0.2, -0.1],
  "metadata": { "text": "Conteúdo" },
  "created_at": 1707123456
}
```

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/points/doc-1
```

---

### POST /api/v1/collections/{name}/search

Busca os pontos mais similares ao vetor de consulta (busca vetorial).

**Path:** `name` — nome da coleção.

**Request body:**

| Campo   | Tipo   | Obrigatório | Descrição |
| ------- | ------ | ----------- | --------- |
| `vector`| array  | sim         | Vetor de consulta (mesma dimensão da coleção) |
| `limit`| number | sim         | Número máximo de resultados (> 0) |
| `filter`| object | não         | Filtro por igualdade em metadata (ex.: `{"source": "manual"}`) |

**Schema de request:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": null
}
```

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "results": [
    {
      "id": "doc-1",
      "score": 0.92,
      "metadata": { "text": "Conteúdo" }
    }
  ],
  "took_ms": 2
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search \
  -H "Content-Type: application/json" \
  -d '{"vector":[0.1,0.2,-0.1],"limit":5}'
```

---

### POST /api/v1/collections/{name}/search/hybrid

Busca híbrida: combina resultados vetoriais e BM25 (keyword) via RRF. A coleção deve ter sido criada com `enable_bm25: true`.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo          | Tipo   | Obrigatório | Descrição |
| -------------- | ------ | ----------- | --------- |
| `query_text`  | string | sim         | Texto para busca keyword (BM25) |
| `query_vector`| array  | sim         | Vetor para busca vetorial |
| `limit`       | number | sim         | Número máximo de resultados (> 0) |
| `alpha`       | number | não         | Peso da busca vetorial 0..1 (default: 0.5). (1 - alpha) = peso keyword |

**Schema de request:**

```json
{
  "query_text": "como fazer deploy",
  "query_vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "alpha": 0.5
}
```

**Resposta:** `200 OK` (mesmo schema de resposta de search)

```json
{
  "results": [
    { "id": "doc-1", "score": 0.85, "metadata": { "text": "..." } }
  ],
  "took_ms": 3
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/hybrid \
  -H "Content-Type: application/json" \
  -d '{"query_text":"deploy","query_vector":[0.1,0.2,-0.1],"limit":5,"alpha":0.5}'
```

---

## Estatísticas

### GET /api/v1/collections/{name}/stats

Retorna estatísticas de uso da coleção (pontos e queries).

**Path:** `name` — nome da coleção.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "num_points": 42,
  "num_queries": 100,
  "avg_latency_ms": 2.5,
  "p50_latency_ms": 2.0,
  "p95_latency_ms": 5.0,
  "p99_latency_ms": 8.0
}
```

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/stats
```
