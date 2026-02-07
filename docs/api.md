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

| Campo     | Tipo   | Descrição                                                                                                                                      |
| --------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `error`   | string | Tipo: `collection_not_found`, `collection_already_exists`, `invalid_payload`, `invalid_dimension`, `internal_error`, `query_profile_not_found` |
| `message` | string | Mensagem legível                                                                                                                               |
| `code`    | number | Código HTTP (400, 404, 409, 500)                                                                                                               |

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

| Campo             | Tipo    | Obrigatório | Descrição                                                        |
| ----------------- | ------- | ----------- | ---------------------------------------------------------------- |
| `name`            | string  | sim         | Nome único: apenas `a-zA-Z0-9_-`                                 |
| `dimension`       | number  | sim         | Dimensão dos vetores (1–4096)                                    |
| `distance`        | string  | sim         | Métrica: `Cosine`, `Euclidean`, `DotProduct`                     |
| `enable_bm25`     | boolean | não         | Habilita índice BM25 para busca híbrida (default: false)         |
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

| Campo    | Tipo  | Descrição                                 |
| -------- | ----- | ----------------------------------------- |
| `points` | array | Lista de 1 a 1000 pontos (objeto abaixo). |

Cada elemento de `points`:

| Campo      | Tipo   | Obrigatório | Descrição                                             |
| ---------- | ------ | ----------- | ----------------------------------------------------- |
| `id`       | string | sim         | ID único do ponto                                     |
| `vector`   | array  | sim         | Array de números (float), dimensão igual à da coleção |
| `metadata` | object | não         | JSON arbitrário (default: `{}`)                       |

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

| Campo       | Tipo   | Obrigatório | Descrição                                                                          |
| ----------- | ------ | ----------- | ---------------------------------------------------------------------------------- |
| `vector`    | array  | sim         | Vetor de consulta (mesma dimensão da coleção)                                      |
| `limit`     | number | sim         | Número máximo de resultados (> 0)                                                  |
| `filter`    | object | não         | Filtro em metadata. Ver [Filtro de metadata](#filtro-de-metadata) abaixo.          |
| `budget_ms` | number | não         | Orçamento máximo em ms. Se a estimativa exceder, retorna 422 sem executar a busca. |

**Schema de request:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": null,
  "budget_ms": 50
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

#### Filtro de metadata

O campo `filter` é um objeto JSON. Cada chave é um campo de metadata; o valor pode ser:

- **Valor direto** — tratado como igualdade (`$eq`): `{"source": "manual"}`.
- **Objeto com operadores** — use `$eq`, `$ne`, `$in`, `$gt`, `$lt`, `$gte`, `$lte`:
  - `$eq`, `$ne`: valor exato (qualquer tipo JSON).
  - `$in`: array de valores permitidos.
  - `$gt`, `$lt`, `$gte`, `$lte`: comparação numérica (o campo no metadata deve ser número).

Múltiplos campos são combinados com **AND**. Exemplo:

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

Busca híbrida: combina resultados vetoriais e BM25 (keyword) via RRF. A coleção deve ter sido criada com `enable_bm25: true`.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo          | Tipo   | Obrigatório | Descrição                                                              |
| -------------- | ------ | ----------- | ---------------------------------------------------------------------- |
| `query_text`   | string | sim         | Texto para busca keyword (BM25)                                        |
| `query_vector` | array  | sim         | Vetor para busca vetorial                                              |
| `limit`        | number | sim         | Número máximo de resultados (> 0)                                      |
| `alpha`        | number | não         | Peso da busca vetorial 0..1 (default: 0.5). (1 - alpha) = peso keyword |

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
  "results": [{ "id": "doc-1", "score": 0.85, "metadata": { "text": "..." } }],
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

### POST /api/v1/collections/{name}/search/explain

Busca vetorial com explicação detalhada de cada resultado. Retorna **por que** cada resultado foi retornado (ou filtrado): score breakdown, avaliação de filtros condição-a-condição, posição no ranking e estatísticas do índice HNSW.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo    | Tipo   | Obrigatório | Descrição                                                                |
| -------- | ------ | ----------- | ------------------------------------------------------------------------ |
| `vector` | array  | sim         | Vetor de consulta (mesma dimensão da coleção)                            |
| `limit`  | number | sim         | Número máximo de resultados (> 0)                                        |
| `filter` | object | não         | Filtro em metadata. Ver [Filtro de metadata](#filtro-de-metadata) acima. |

**Schema de request:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": { "category": "tech" }
}
```

**Resposta:** `200 OK`

**Schema de resposta:**

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
  }
}
```

| Campo                          | Tipo   | Descrição                                                       |
| ------------------------------ | ------ | --------------------------------------------------------------- |
| `query_vector_norm`            | number | Norma L2 do vetor de consulta                                   |
| `distance_metric`              | string | Métrica: `Cosine`, `Euclidean`, `DotProduct`                    |
| `candidates_scanned`           | number | Total de candidatos escaneados pelo índice                      |
| `candidates_after_filter`      | number | Candidatos que passaram no filtro                               |
| `results`                      | array  | Resultados explicados individualmente                           |
| `results[].score_breakdown`    | object | Componentes do score (`vector_score`, etc.)                     |
| `results[].filter_evaluation`  | object | Avaliação detalhada do filtro (presente se filtro foi aplicado) |
| `results[].rank_before_filter` | number | Posição no ranking antes de filtros (1-indexed)                 |
| `results[].rank_after_filter`  | number | Posição após filtros (1-indexed, 0 se não passou)               |
| `index_stats`                  | object | Estatísticas do índice HNSW no momento da busca                 |

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/explain \
  -H "Content-Type: application/json" \
  -d '{"vector":[0.1,0.2,-0.1],"limit":5,"filter":{"category":"tech"}}'
```

---

### POST /api/v1/collections/{name}/search/estimate

Estima o custo de uma busca vetorial **antes de executá-la**. Retorna latência estimada, consumo de memória, nós HNSW que serão visitados, se a query é "cara" e recomendações de otimização. Não executa nenhuma busca real.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo             | Tipo    | Obrigatório | Descrição                                                        |
| ----------------- | ------- | ----------- | ---------------------------------------------------------------- |
| `limit`           | number  | sim         | Número de resultados que serão solicitados na busca              |
| `filter`          | object  | não         | Filtro de metadata (mesmo formato do endpoint de busca)          |
| `include_history` | boolean | não         | Se `true`, inclui dados históricos de latência (p50/p95/p99/avg) |

**Schema de request:**

```json
{
  "limit": 10,
  "filter": { "category": "tech" },
  "include_history": true
}
```

**Resposta:** `200 OK`

**Schema de resposta:**

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

| Campo                     | Tipo    | Descrição                                                                   |
| ------------------------- | ------- | --------------------------------------------------------------------------- |
| `estimated_ms`            | number  | Latência estimada em milissegundos                                          |
| `confidence_range`        | array   | Faixa de confiança `[min, max]` em ms                                       |
| `estimated_memory_bytes`  | number  | Bytes estimados de memória que a query consumirá                            |
| `estimated_nodes_visited` | number  | Nós HNSW estimados que serão visitados                                      |
| `is_expensive`            | boolean | `true` se a estimativa excede o p95 histórico                               |
| `recommendations`         | array   | Sugestões de otimização (ex: "reduza limit")                                |
| `breakdown`               | object  | Componentes individuais do custo (index_scan, filter, hydration, network)   |
| `historical_latency`      | object  | Presente se `include_history=true`. Percentis e média de latência histórica |

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/estimate \
  -H "Content-Type: application/json" \
  -d '{"limit":10,"filter":{"category":"tech"},"include_history":true}'
```

#### Budget-Based Queries

O campo `budget_ms` no endpoint `POST /search` permite definir um orçamento máximo de latência. Se a estimativa de custo exceder o orçamento, a busca **não é executada** e o servidor retorna `422 Unprocessable Entity` com a estimativa detalhada:

```json
{
  "error": "budget_exceeded",
  "message": "estimated cost (12.5ms) exceeds budget (5ms)",
  "code": 422,
  "estimate": {
    "estimated_ms": 12.5,
    "confidence_range": [6.25, 18.75],
    "is_expensive": true,
    "recommendations": ["Considere reduzir limit para melhor performance"],
    "breakdown": { "...": "..." }
  }
}
```

---

## Dashboard

### GET /dashboard

Retorna a página HTML do dashboard (single-file com Alpine.js, Tailwind CDN e Chart.js). Exibe lista de coleções com stats, gráfico de queries/min (24h), top 10 queries mais lentas (`GET /api/v1/stats/slow-queries`) e distribuição de latências. Atualiza via polling a cada 5s usando `GET /api/v1/stats/global`, `GET /api/v1/stats/queries`, `GET /api/v1/stats/slow-queries` e `GET /api/v1/collections/{name}/stats`.

**Resposta:** `200 OK` (HTML)

---

## Estatísticas e analytics

Os endpoints de analytics leem o arquivo `queries.log` (JSONL) e mantêm cache em memória por 1h.

### GET /api/v1/stats/global

Retorna estatísticas globais: totais do servidor (coleções, pontos) e agregados das últimas 24h a partir do log de queries.

**Resposta:** `200 OK`

**Schema de resposta:**

```json
{
  "total_collections": 2,
  "total_points": 150,
  "total_queries_24h": 420,
  "avg_latency_ms": 3.5,
  "queries_per_minute": [{ "timestamp": 1738742400, "count": 12 }]
}
```

| Campo                | Tipo   | Descrição                                                 |
| -------------------- | ------ | --------------------------------------------------------- |
| `total_collections`  | number | Número de coleções                                        |
| `total_points`       | number | Soma de pontos em todas as coleções                       |
| `total_queries_24h`  | number | Queries nas últimas 24h (do log)                          |
| `avg_latency_ms`     | number | Latência média (ms) nas últimas 24h                       |
| `queries_per_minute` | array  | Buckets por minuto: `timestamp` (Unix do minuto), `count` |

---

### GET /api/v1/stats/queries

Lista queries das últimas 24h (lê de `queries.log`, cache 1h).

**Query params:**

| Param        | Tipo   | Default | Descrição                                               |
| ------------ | ------ | ------- | ------------------------------------------------------- |
| `collection` | string | —       | Filtrar por nome da coleção                             |
| `limit`      | number | 100     | Máximo de entradas                                      |
| `sort`       | string | —       | `latency` = ordenar por latência (mais lentas primeiro) |

**Resposta:** `200 OK`

**Schema de resposta:** array de objetos:

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

Queries com latência acima do threshold (lê de `queries.log`, cache 1h).

**Query params:**

| Param          | Tipo   | Default | Descrição            |
| -------------- | ------ | ------- | -------------------- |
| `threshold_ms` | number | 100     | Latência mínima (ms) |
| `limit`        | number | 10      | Máximo de entradas   |

**Resposta:** `200 OK` — mesmo schema de array que `GET /api/v1/stats/queries`.

---

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

---

## Debug (query profiling)

### GET /api/v1/debug/query-profile/{query_id}

Retorna o perfil de execução de uma query (tempo por fase: validação, busca, hydrate). Útil para debug de performance quando a resposta de search inclui `query_id`.

**Path:** `query_id` — UUID retornado em `SearchPointsResponse.query_id`.

**Resposta:** `200 OK`

**Schema de resposta:**

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

| Campo                  | Tipo   | Descrição                                  |
| ---------------------- | ------ | ------------------------------------------ |
| `query_id`             | string | ID da query                                |
| `total_ms`             | number | Tempo total em ms                          |
| `phases`               | array  | Tempo por fase                             |
| `phases[].name`        | string | Nome da fase (validation, search, hydrate) |
| `phases[].duration_ms` | number | Duração da fase em ms                      |
| `phases[].percentage`  | number | Percentual do total                        |

**Erro:** `404` com `error: "query_profile_not_found"` se o `query_id` não existir (perfis são mantidos em memória com capacidade limitada).

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/debug/query-profile/550e8400-e29b-41d4-a716-446655440000
```
