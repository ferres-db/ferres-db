# Referência da API HTTP — FerresDB

Referência dos endpoints REST do servidor FerresDB. Base URL de exemplo: `http://localhost:8080`.

## Requisitos de CPU para performance máxima

Para melhor throughput em buscas vetoriais, o servidor utiliza kernels SIMD quando disponíveis. Recomenda-se CPU com suporte a **AVX2** ou, na falta, **SSE4.1**, para performance máxima nas operações de distância (Euclidean, DotProduct, Cosine) e no re-ranking com quantização SQ8. Em CPUs sem essas instruções, o código usa implementação escalar (comportamento correto, com performance menor). A detecção é automática em tempo de execução; não é necessária configuração.

---

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
| `tiered_storage`  | object  | não         | Configuração de tiered storage (ver seção Tiered Storage)        |

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

### GET /api/v1/collections/{name}/tiers

Retorna a distribuição de pontos por camada de armazenamento (Tiered Storage).

**Path:** `name` — nome da coleção.

**Resposta:** `200 OK`

**Schema de resposta:**

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

| Campo               | Tipo   | Descrição                                          |
| ------------------- | ------ | -------------------------------------------------- |
| `hot`               | number | Pontos na camada Hot (RAM, acesso instantâneo)     |
| `warm`              | number | Pontos na camada Warm (mmap, acesso rápido)        |
| `cold`              | number | Pontos na camada Cold (disco, carregado on-demand) |
| `hot_memory_bytes`  | number | Memória estimada usada pela camada Hot (bytes)     |
| `warm_memory_bytes` | number | Memória estimada usada pela camada Warm (bytes)    |
| `cold_memory_bytes` | number | Memória estimada usada pela camada Cold (bytes)    |

**Exemplo curl:**

```bash
curl -s http://localhost:8080/api/v1/collections/docs/tiers \
  -H "Authorization: Bearer <api-key>"
```

> **Nota:** Quando tiered storage não está habilitado na coleção, todos os pontos
> são reportados na camada Hot. Habilite via `tiered_storage` na criação da coleção.

---

### Tiered Storage (Configuração)

O Tiered Storage é configurado via campo `tiered_storage` na criação da coleção:

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

| Campo                      | Tipo    | Default | Descrição                                                 |
| -------------------------- | ------- | ------- | --------------------------------------------------------- |
| `enabled`                  | boolean | false   | Habilita tiered storage                                   |
| `hot_threshold_hours`      | number  | 24      | Pontos acessados nas últimas N horas ficam em Hot (RAM)   |
| `warm_threshold_hours`     | number  | 168     | Pontos acessados nas últimas N horas ficam em Warm (mmap) |
| `compaction_interval_secs` | number  | 3600    | Intervalo entre compactações automáticas (segundos)       |

**Camadas:**

| Tier | Armazenamento             | Latência | Memória |
| ---- | ------------------------- | -------- | ------- |
| Hot  | RAM completa              | ~0ms     | Alta    |
| Warm | mmap (vetor) + RAM (meta) | ~1ms     | Média   |
| Cold | Disco (on-demand)         | ~5-10ms  | Mínima  |

**Comportamento:**

- O grafo HNSW **sempre** permanece em memória (apenas dados dos pontos são tiered).
- Qualquer acesso a um ponto Cold/Warm o promove automaticamente para Hot.
- A compactação roda em background sem bloquear buscas.
- Quando desabilitado (default), tudo fica em RAM como antes.

---

## Reindex (Background Index Rebuild)

Reconstrói o índice ANN de uma coleção em background sem bloquear buscas nem mutações. Remove tombstones acumulados e restaura performance de busca.

**Fluxo:**

1. **Building**: Snapshot dos pontos → novo índice construído em thread separada. Buscas continuam no índice antigo.
2. **Swapping**: Write lock < 1ms para trocar índice antigo pelo novo. Delta (ops durante build) é aplicado.
3. **Cleanup**: Índice antigo é descartado (memória liberada).

**Auto-reindex**: Dispara automaticamente quando tombstones > 20% dos pontos indexados (após deleções).

### POST /api/v1/collections/{name}/reindex

Inicia um job de reindex em background. Apenas 1 job ativo por coleção.

**Resposta:** `202 Accepted`

```json
{
  "job_id": "550e8400-e29b-41d4-a716-446655440000",
  "collection": "my-vectors",
  "status": "Building",
  "message": "reindex job started"
}
```

| Campo        | Tipo   | Descrição                   |
| ------------ | ------ | --------------------------- |
| `job_id`     | string | UUID do job criado          |
| `collection` | string | Nome da coleção             |
| `status`     | string | Status inicial (`Building`) |
| `message`    | string | Mensagem descritiva         |

**Erros:**

- `404` — coleção não encontrada.
- `409` — já existe um job de reindex ativo para esta coleção.

**Exemplo:**

```bash
curl -X POST http://localhost:8080/api/v1/collections/my-vectors/reindex \
  -H "Authorization: Bearer sk-xxx"
```

### GET /api/v1/collections/{name}/reindex/{job_id}

Retorna o status de um job de reindex específico.

**Resposta:** `200 OK`

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

| Campo                        | Tipo    | Descrição                                               |
| ---------------------------- | ------- | ------------------------------------------------------- |
| `id`                         | string  | UUID do job                                             |
| `collection`                 | string  | Nome da coleção                                         |
| `status`                     | string  | `Queued`, `Building`, `Swapping`, `Completed`, `Failed` |
| `progress`                   | number  | Progresso de 0.0 a 1.0                                  |
| `started_at`                 | number  | Timestamp UNIX (segundos)                               |
| `completed_at`               | number? | Timestamp de conclusão (null se em andamento)           |
| `error`                      | string? | Mensagem de erro (null se sucesso)                      |
| `stats.points_processed`     | number  | Pontos processados                                      |
| `stats.points_total`         | number  | Total de pontos no snapshot                             |
| `stats.tombstones_cleaned`   | number  | Tombstones removidos                                    |
| `stats.old_index_size_bytes` | number  | Tamanho estimado do índice antigo (bytes)               |
| `stats.new_index_size_bytes` | number  | Tamanho estimado do novo índice (bytes)                 |

**Erros:**

- `404` — coleção ou job não encontrado.

**Exemplo:**

```bash
curl http://localhost:8080/api/v1/collections/my-vectors/reindex/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer sk-xxx"
```

### GET /api/v1/collections/{name}/reindex

Lista todos os jobs de reindex de uma coleção (mais recentes primeiro).

**Resposta:** `200 OK`

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

**Erros:**

- `404` — coleção não encontrada.

**Exemplo:**

```bash
curl http://localhost:8080/api/v1/collections/my-vectors/reindex \
  -H "Authorization: Bearer sk-xxx"
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

| Campo       | Tipo   | Obrigatório | Descrição                                                                                        |
| ----------- | ------ | ----------- | ------------------------------------------------------------------------------------------------ |
| `id`        | string | sim         | ID único do ponto                                                                                |
| `vector`    | array  | sim         | Array de números (float), dimensão igual à da coleção                                            |
| `metadata`  | object | não         | JSON arbitrário (default: `{}`)                                                                  |
| `namespace` | string | não         | Namespace lógico (multitenancy). Quando omitido, o ponto não tem namespace.                      |
| `ttl`       | number | não         | TTL em segundos; se presente, o ponto expira após esse tempo e é removido pelo worker de vacuum. |

**Schema de request:**

```json
{
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "metadata": { "text": "Conteúdo do documento" },
      "ttl": 3600
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

| Campo       | Tipo   | Obrigatório | Descrição                                                                   |
| ----------- | ------ | ----------- | --------------------------------------------------------------------------- |
| `ids`       | array  | sim         | Lista de IDs dos pontos a remover                                           |
| `namespace` | string | não         | Quando informado, remove apenas pontos desse namespace (para os ids dados). |

```json
{
  "ids": ["doc-1", "doc-2"],
  "namespace": "tenant-a"
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

**Query params:**

| Campo       | Tipo   | Descrição                                                              |
| ----------- | ------ | ---------------------------------------------------------------------- |
| `namespace` | string | Quando o ponto foi inserido com namespace, informe-o para localização. |

**Resposta:** `200 OK`. Inclui o campo `namespace` quando o ponto tiver namespace.

**Schema de resposta:**

```json
{
  "id": "doc-1",
  "vector": [0.1, 0.2, -0.1],
  "metadata": { "text": "Conteúdo" },
  "created_at": 1707123456,
  "namespace": "tenant-a"
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
| `namespace` | string | não         | Restringe resultados a este namespace (multitenancy).                              |
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

**Schema de resposta:** cada resultado pode incluir `namespace` quando o ponto tiver namespace.

```json
{
  "results": [
    {
      "id": "doc-1",
      "score": 0.92,
      "metadata": { "text": "Conteúdo" },
      "namespace": "tenant-a"
    }
  ],
  "took_ms": 2
}
```

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search \
  -H "Content-Type: application/json" \
  -d '{"vector":[0.1,0.2,-0.1],"limit":5,"namespace":"tenant-a"}'
```

#### Filtro de metadata

Quando o campo `filter` é informado, a busca utiliza **pre-filtering nativo no HNSW**: o filtro é aplicado durante a exploração do grafo (não após a busca), garantindo maior precisão e consistência no número de resultados retornados (até `limit` que satisfazem o filtro), sem depender de multiplicador fixo.

O campo `filter` é um objeto JSON. Cada chave é um campo de metadata; o valor pode ser:

- **Valor direto** — tratado como igualdade (`$eq`): `{"source": "manual"}`.
- **Objeto com operadores** — use `$eq`, `$ne`, `$in`, `$gt`, `$lt`, `$gte`, `$lte`:
  - `$eq`, `$ne`: valor exato (qualquer tipo JSON).
  - `$in`: array de valores permitidos.
  - `$gt`, `$lt`, `$gte`, `$lte`: comparação numérica (o campo no metadata deve ser número).
- **Chave reservada `$namespace`** — restringe resultados ao namespace indicado (multitenancy): `{"$namespace": "tenant-id"}`. Pode ser combinada com outras condições.

Alternativamente, use o parâmetro de primeiro nível `namespace` no body da busca em vez de `$namespace` no filter.

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

Busca híbrida: combina resultados vetoriais e BM25 (keyword) via estratégia de fusão configurável. A coleção deve ter sido criada com `enable_bm25: true`.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo          | Tipo   | Obrigatório | Descrição                                                                                                            |
| -------------- | ------ | ----------- | -------------------------------------------------------------------------------------------------------------------- |
| `query_text`   | string | sim         | Texto para busca keyword (BM25)                                                                                      |
| `query_vector` | array  | sim         | Vetor para busca vetorial                                                                                            |
| `limit`        | number | sim         | Número máximo de resultados (> 0)                                                                                    |
| `alpha`        | number | não         | Peso da busca vetorial 0..1 (default: 0.5). (1 - alpha) = peso keyword. Usado com `fusion: "weighted"`               |
| `fusion`       | string | não         | Estratégia de fusão: `"weighted"` (default) ou `"rrf"`                                                               |
| `rrf_k`        | number | não         | Constante k para RRF (default: 60). Apenas usado quando `fusion: "rrf"`. Valores maiores suavizam diferenças de rank |
| `namespace`    | string | não         | Restringe resultados a este namespace (multitenancy)                                                                 |

**Estratégias de fusão:**

- **`weighted`** (default): Pondera os rankings por `alpha`. Score = `alpha × 1/(k + rank_vec) + (1-alpha) × 1/(k + rank_bm25)`. Permite controlar o balanço entre busca vetorial e keyword.
- **`rrf`** (Reciprocal Rank Fusion): Fusão pura por rank sem ponderação. Score = `Σ 1/(k + rank_i)` para cada ranker. Produz resultados mais estáveis pois trata todos os rankers igualmente e não depende da escala dos scores originais.

**Quando usar RRF vs Weighted:**

- Use **weighted** quando quiser controlar manualmente o balanço entre vetorial e keyword via `alpha`.
- Use **rrf** quando quiser resultados mais estáveis, independentes da escala dos scores de cada ranker.

**Schema de request (weighted — default):**

```json
{
  "query_text": "como fazer deploy",
  "query_vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "alpha": 0.5
}
```

**Schema de request (RRF):**

```json
{
  "query_text": "como fazer deploy",
  "query_vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "fusion": "rrf",
  "rrf_k": 60
}
```

**Resposta:** `200 OK` (mesmo schema de resposta de search)

```json
{
  "results": [{ "id": "doc-1", "score": 0.85, "metadata": { "text": "..." } }],
  "took_ms": 3
}
```

**Exemplo curl (weighted):**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/hybrid \
  -H "Content-Type: application/json" \
  -d '{"query_text":"deploy","query_vector":[0.1,0.2,-0.1],"limit":5,"alpha":0.5}'
```

**Exemplo curl (RRF):**

```bash
curl -s -X POST http://localhost:8080/api/v1/collections/docs/search/hybrid \
  -H "Content-Type: application/json" \
  -d '{"query_text":"deploy","query_vector":[0.1,0.2,-0.1],"limit":5,"fusion":"rrf","rrf_k":60}'
```

---

### POST /api/v1/collections/{name}/search/explain

Busca vetorial com explicação detalhada de cada resultado. Retorna **por que** cada resultado foi retornado (ou filtrado): score breakdown, avaliação de filtros condição-a-condição, posição no ranking e estatísticas do índice HNSW.

**Path:** `name` — nome da coleção.

**Request body:**

| Campo       | Tipo   | Obrigatório | Descrição                                                                |
| ----------- | ------ | ----------- | ------------------------------------------------------------------------ |
| `vector`    | array  | sim         | Vetor de consulta (mesma dimensão da coleção)                            |
| `limit`     | number | sim         | Número máximo de resultados (> 0)                                        |
| `filter`    | object | não         | Filtro em metadata. Ver [Filtro de metadata](#filtro-de-metadata) acima. |
| `namespace` | string | não         | Restringe resultados a este namespace (multitenancy)                     |

**Schema de request:**

```json
{
  "vector": [0.1, 0.2, -0.1],
  "limit": 5,
  "filter": { "category": "tech" },
  "namespace": "tenant-a"
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
| `namespace`       | string  | não         | Restringe a estimativa ao cenário com filtro por este namespace  |
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

**Auto-reindex em background:** Um worker interno percorre todas as coleções a cada 30 minutos e, quando o rácio de tombstones (`tombstone_count / total_indexed`) excede 20%, dispara um reindex automático (mesma lógica de swap de índice dos endpoints de reindex). O estado do worker não é exposto em nenhum endpoint de stats; a observabilidade é feita via logs estruturados (`tracing`): início e fim de cada ciclo do worker e início e fim de cada compactação.

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

---

## RBAC — Controle de Acesso Granular

O FerresDB suporta controle de acesso baseado em roles (RBAC) com permissões granulares por coleção e restrições de metadata.

### Modelo de Permissões

Cada usuário possui um `role` (Admin, Editor, Viewer) e opcionalmente `permissions` granulares. Se `permissions` estiver configurado, tem precedência sobre o role legado.

**Recursos:**

| Tipo              | Descrição                     |
| ----------------- | ----------------------------- |
| `all_collections` | Wildcard: todas as coleções   |
| `collection`      | Coleção específica (por nome) |

**Ações:**

| Ação     | Descrição                      |
| -------- | ------------------------------ |
| `read`   | search, get, list              |
| `write`  | upsert, delete pontos          |
| `create` | criar coleção                  |
| `delete` | deletar coleção                |
| `admin`  | gerenciar usuários, save, etc. |

**Restrição de Metadata:**

Opcional. Quando presente, resultados de busca são filtrados automaticamente (AND com filtros do request). Garante isolamento de dados por equipe/departamento.

### POST /api/v1/users (com permissões)

Cria um usuário com permissões granulares.

**Request body (exemplo com permissões):**

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

**Exemplo curl:**

```bash
curl -s -X POST http://localhost:8080/api/v1/users \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <admin-key>" \
  -d '{"username":"analyst","password":"secret","role":"viewer","permissions":[{"resource":{"type":"all_collections"},"actions":["read"]}]}'
```

---

### PUT /api/v1/users/{username}/permissions

Atualiza permissões granulares de um usuário (apenas Admin).

**Path:** `username` — nome do usuário.

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

Para remover permissões granulares (voltar ao comportamento legado de role):

```json
{
  "permissions": null
}
```

**Resposta:** `200 OK`

```json
{
  "updated": true,
  "username": "analyst"
}
```

**Exemplo curl:**

```bash
curl -s -X PUT http://localhost:8080/api/v1/users/analyst/permissions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <admin-key>" \
  -d '{"permissions":[{"resource":{"type":"collection","name":"docs"},"actions":["read","write"]}]}'
```

---

### Enforcement de Permissões

| Endpoint                                 | Permissão Necessária |
| ---------------------------------------- | -------------------- |
| POST /collections                        | `create`             |
| DELETE /collections/{name}               | `delete`             |
| POST /collections/{name}/points          | `write`              |
| DELETE /collections/{name}/points        | `write`              |
| POST /collections/{name}/search          | `read`               |
| POST /collections/{name}/search/hybrid   | `read`               |
| POST /collections/{name}/search/explain  | `read`               |
| POST /collections/{name}/search/estimate | `read`               |
| GET /api/v1/audit                        | Admin only           |

**Regras de precedência:**

1. **Admin** → sempre permitido
2. **Permissões granulares** → verificadas se configuradas
3. **Role legado** → Editor pode read/write/create; Viewer pode read

**MetadataRestriction:** Se o usuário tem uma `metadata_restriction` na permissão de Read, o filtro é injetado automaticamente (AND com filtros do request). Exemplo: um usuário com `department=sales` só verá resultados com `department=sales`.

---

## Audit Trail

O FerresDB registra todas as ações em um audit trail persistente (arquivos JSONL com rotação diária).

### GET /api/v1/audit

Consulta entradas de auditoria filtradas (apenas Admin).

**Query params:**

| Param      | Tipo   | Default | Descrição                                              |
| ---------- | ------ | ------- | ------------------------------------------------------ |
| `user`     | string | —       | Filtrar por user_id                                    |
| `action`   | string | —       | Filtrar por ação (ex: "search", "upsert", "login")     |
| `resource` | string | —       | Filtrar por recurso (substring, ex: "collection:docs") |
| `from`     | string | 7d ago  | Data/hora de início (RFC 3339)                         |
| `to`       | string | now     | Data/hora de fim (RFC 3339)                            |
| `limit`    | number | 100     | Máximo de entradas (max: 1000)                         |

**Resposta:** `200 OK`

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

| Campo         | Tipo   | Descrição                                                                                           |
| ------------- | ------ | --------------------------------------------------------------------------------------------------- |
| `timestamp`   | string | Data/hora UTC (RFC 3339)                                                                            |
| `user_id`     | string | Usuário que executou a ação                                                                         |
| `action`      | string | Ação: search, upsert, delete_points, create_collection, delete_collection, login, create_user, etc. |
| `resource`    | string | Recurso: collection:nome, user:nome, api_key:nome, system:collections                               |
| `details`     | object | Detalhes resumidos (sem vetores)                                                                    |
| `result`      | string | success, denied, error                                                                              |
| `ip_address`  | string | IP do cliente (se disponível)                                                                       |
| `duration_ms` | number | Duração em ms (se disponível)                                                                       |

**Ações auditadas:**

- `login` (sucesso e falha)
- `search`, `search_hybrid`
- `upsert`, `delete_points`
- `create_collection`, `delete_collection`
- `create_user`, `delete_user`, `update_password`, `update_permissions`
- `create_api_key`, `delete_api_key`
- `save`

**Exemplo curl:**

```bash
# Todas as ações das últimas 24h
curl -s http://localhost:8080/api/v1/audit?limit=50 \
  -H "Authorization: Bearer <admin-key>"

# Ações de um usuário específico
curl -s "http://localhost:8080/api/v1/audit?user=analyst&action=search" \
  -H "Authorization: Bearer <admin-key>"

# Ações negadas
curl -s "http://localhost:8080/api/v1/audit?from=2026-02-07T00:00:00Z&to=2026-02-08T00:00:00Z" \
  -H "Authorization: Bearer <admin-key>"
```

### Armazenamento

Os logs de auditoria são escritos em arquivos JSONL no diretório de dados:

```
data/logs/audit-2026-02-07.jsonl
data/logs/audit-2026-02-08.jsonl
```

- Rotação diária (novo arquivo por dia)
- Append-only (como o WAL)
- Escrita assíncrona (não impacta latência dos handlers)

---

## WebSocket Streaming

O FerresDB suporta ingestão e subscrição de eventos em tempo real via WebSocket. O protocolo é JSON sobre WebSocket com mensagens tipadas.

### GET /api/v1/ws

Endpoint para upgrade HTTP → WebSocket.

**Autenticação:** Obrigatória. Aceita API key de duas formas:

- **Query param:** `?token=sk-xxx`
- **Header:** `Authorization: Bearer <key>`

**Limites:**

| Parâmetro                  | Valor                                  |
| -------------------------- | -------------------------------------- |
| Máximo de conexões         | 100 (configurável)                     |
| Tamanho máximo de mensagem | 10 MB                                  |
| Heartbeat (ping)           | a cada 30s                             |
| Timeout de pong            | 10s (se não chegar, conexão é fechada) |
| Timeout de inatividade     | 5 minutos                              |
| Debounce de batch (upsert) | 10ms                                   |

**Exemplo de conexão (JavaScript):**

```javascript
const ws = new WebSocket("ws://localhost:8080/api/v1/ws?token=sk-xxx");
ws.onopen = () => console.log("Connected");
ws.onmessage = (event) => console.log(JSON.parse(event.data));
```

**Exemplo com wscat:**

```bash
wscat -c "ws://localhost:8080/api/v1/ws?token=sk-xxx"
```

**Erros de conexão:**

| Status | Descrição                               |
| ------ | --------------------------------------- |
| `401`  | API key inválida ou ausente             |
| `503`  | Limite de conexões simultâneas atingido |

---

### Protocolo de Mensagens

Todas as mensagens são objetos JSON com campo `type` discriminador.

#### Mensagens do Cliente → Servidor

##### `upsert` — Ingestão de pontos em tempo real

Insere ou atualiza pontos numa coleção. Mensagens de upsert são acumuladas internamente por 10ms (debounce) antes do flush para melhor throughput em alta frequência.

```json
{
  "type": "upsert",
  "collection": "my_collection",
  "points": [
    {
      "id": "doc-1",
      "vector": [0.1, 0.2, -0.1],
      "metadata": { "text": "conteúdo do documento" }
    },
    {
      "id": "doc-2",
      "vector": [0.3, -0.1, 0.5],
      "metadata": { "text": "outro documento" }
    }
  ]
}
```

| Campo        | Tipo   | Obrigatório | Descrição                              |
| ------------ | ------ | ----------- | -------------------------------------- |
| `type`       | string | sim         | Sempre `"upsert"`                      |
| `collection` | string | sim         | Nome da coleção alvo                   |
| `points`     | array  | sim         | Array de pontos (id, vector, metadata) |

Cada ponto:

| Campo      | Tipo   | Obrigatório | Descrição                                   |
| ---------- | ------ | ----------- | ------------------------------------------- |
| `id`       | string | sim         | ID único do ponto                           |
| `vector`   | array  | sim         | Array de floats (mesma dimensão da coleção) |
| `metadata` | object | não         | JSON arbitrário (default: `{}`)             |

**Resposta:** mensagem `ack` (ver abaixo).

##### `subscribe` — Subscrição a eventos de uma coleção

Inscreve a conexão para receber notificações em tempo real quando pontos são inseridos ou deletados numa coleção.

```json
{
  "type": "subscribe",
  "collection": "my_collection",
  "events": ["upsert", "delete"]
}
```

| Campo        | Tipo   | Obrigatório | Descrição                                                                                        |
| ------------ | ------ | ----------- | ------------------------------------------------------------------------------------------------ |
| `type`       | string | sim         | Sempre `"subscribe"`                                                                             |
| `collection` | string | sim         | Nome da coleção para subscrever                                                                  |
| `events`     | array  | não         | Filtro de tipos de evento: `["upsert"]`, `["delete"]`, ou ambos. Se vazio/ausente, recebe todos. |

**Resposta:** mensagem `ack` confirmando a subscrição (com `upserted: 0, failed: 0, took_ms: 0`).

**Erros:**

- `404` — coleção não encontrada
- `409` — já está subscrito nesta coleção

##### `ping` — Heartbeat aplicacional

```json
{
  "type": "ping"
}
```

**Resposta:** mensagem `pong`.

---

#### Mensagens do Servidor → Cliente

##### `ack` — Confirmação de operação

Enviada após um `upsert` ou `subscribe` bem-sucedido.

```json
{
  "type": "ack",
  "upserted": 10,
  "failed": 0,
  "took_ms": 5
}
```

| Campo      | Tipo   | Descrição                                     |
| ---------- | ------ | --------------------------------------------- |
| `type`     | string | Sempre `"ack"`                                |
| `upserted` | number | Pontos inseridos/atualizados com sucesso      |
| `failed`   | number | Pontos que falharam (dimensão inválida, etc.) |
| `took_ms`  | number | Tempo de processamento em ms                  |

##### `event` — Notificação de mudança em coleção

Enviada para subscribers quando pontos são inseridos ou deletados (via REST ou WebSocket).

```json
{
  "type": "event",
  "collection": "my_collection",
  "action": "upsert",
  "point_ids": ["doc-1", "doc-2"],
  "timestamp": 1707123456
}
```

| Campo        | Tipo   | Descrição                                  |
| ------------ | ------ | ------------------------------------------ |
| `type`       | string | Sempre `"event"`                           |
| `collection` | string | Nome da coleção                            |
| `action`     | string | Tipo de operação: `"upsert"` ou `"delete"` |
| `point_ids`  | array  | IDs dos pontos afetados                    |
| `timestamp`  | number | Timestamp UNIX (segundos) da operação      |

**Nota:** Eventos são emitidos tanto por operações REST (`POST /points`, `DELETE /points`) quanto por upserts via WebSocket. Todos os subscribers ativos recebem a notificação.

##### `error` — Erro

```json
{
  "type": "error",
  "message": "collection not found",
  "code": 404
}
```

| Campo     | Tipo   | Descrição                                       |
| --------- | ------ | ----------------------------------------------- |
| `type`    | string | Sempre `"error"`                                |
| `message` | string | Mensagem legível do erro                        |
| `code`    | number | Código HTTP semântico (400, 404, 408, 409, 500) |

Códigos de erro comuns:

| Code | Descrição                            |
| ---- | ------------------------------------ |
| 400  | Mensagem JSON inválida ou malformada |
| 404  | Coleção não encontrada               |
| 408  | Timeout (inatividade ou pong)        |
| 409  | Já subscrito nesta coleção           |
| 500  | Erro interno (lock, insert, etc.)    |

##### `pong` — Resposta a ping

```json
{
  "type": "pong"
}
```

---

### Comportamento de Batch (Debounce)

Mensagens `upsert` enviadas em rápida sucessão são acumuladas automaticamente por **10ms** antes de serem processadas. Isso melhora significativamente o throughput em cenários de alta frequência de ingestão:

1. O cliente envia múltiplas mensagens `upsert` rapidamente
2. O servidor acumula todas durante a janela de 10ms
3. Pontos são agrupados por coleção
4. Cada grupo é inserido em batch único
5. Um `ack` é enviado por coleção com o total consolidado

---

### Heartbeat e Timeouts

O servidor mantém a conexão saudável com heartbeat bidirecional:

1. **Ping do servidor** (a cada 30s): o servidor envia `{"type":"ping"}` ao cliente
2. **Pong do cliente**: o cliente deve responder com `{"type":"ping"}` (que recebe `{"type":"pong"}`)
3. **Timeout de pong**: se o pong não chegar dentro de um intervalo de heartbeat (~30s), a conexão é fechada com `{"type":"error","message":"pong timeout","code":408}`
4. **Timeout de inatividade**: se não houver atividade (nenhuma mensagem recebida) por 5 minutos, a conexão é fechada com `{"type":"error","message":"inactivity timeout","code":408}`

---

### Métricas Prometheus

O WebSocket expõe métricas para observabilidade:

| Métrica                      | Tipo    | Labels | Descrição                                           |
| ---------------------------- | ------- | ------ | --------------------------------------------------- |
| `ws_connections_active`      | Gauge   | —      | Conexões WebSocket ativas no momento                |
| `ws_messages_received_total` | Counter | `type` | Mensagens recebidas (text, upsert, subscribe, ping) |
| `ws_messages_sent_total`     | Counter | `type` | Mensagens enviadas (outgoing, event)                |

---

### Exemplo Completo: Ingestão + Subscrição

```javascript
// Conectar
const ws = new WebSocket("ws://localhost:8080/api/v1/ws?token=sk-xxx");

ws.onopen = () => {
  // 1. Subscrever a eventos da coleção "docs"
  ws.send(
    JSON.stringify({
      type: "subscribe",
      collection: "docs",
      events: ["upsert", "delete"],
    }),
  );

  // 2. Inserir pontos via WebSocket
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

// Heartbeat: responder pings do servidor
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === "ping") {
    ws.send(JSON.stringify({ type: "ping" }));
  }
};
```

**Exemplo Python (websockets):**

```python
import asyncio
import json
import websockets

async def main():
    uri = "ws://localhost:8080/api/v1/ws?token=sk-xxx"
    async with websockets.connect(uri) as ws:
        # Subscrever
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

        # Receber mensagens
        async for message in ws:
            msg = json.loads(message)
            print(f"[{msg['type']}] {msg}")

asyncio.run(main())
```

---

## API gRPC (feature `grpc`)

O FerresDB oferece uma **API gRPC nativa** como alternativa à API REST, ideal para comunicação server-to-server com alta performance, streaming bidirecional e geração automática de clientes em qualquer linguagem.

### Ativação

A API gRPC é opcional e controlada por feature flag. Para compilar com suporte gRPC:

```bash
cargo build -p ferres-db-server --features grpc
```

O servidor REST continua funcionando normalmente mesmo sem a feature `grpc`.

### Portas

| Protocolo | Porta padrão | Configuração         |
| --------- | ------------ | -------------------- |
| REST/HTTP | 8080         | `PORT` env ou config |
| gRPC      | 50051        | `GRPC_PORT` env      |

Ambos os servidores rodam simultaneamente quando a feature está habilitada.

### Proto file

O arquivo de definição está em `crates/server/proto/ferresdb.proto` (package `ferresdb.v1`).

### Serviço `FerresDB`

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

  // Streaming bidirecional
  rpc StreamUpsert(stream UpsertPointsRequest) returns (stream UpsertPointsResponse);
  rpc StreamSearch(stream SearchRequest) returns (stream SearchResponse);
}
```

### Mapeamento REST → gRPC

| REST Endpoint                                    | gRPC RPC                        |
| ------------------------------------------------ | ------------------------------- |
| `POST /api/v1/collections`                       | `CreateCollection`              |
| `GET  /api/v1/collections`                       | `ListCollections`               |
| `GET  /api/v1/collections/{name}`                | `GetCollection`                 |
| `DELETE /api/v1/collections/{name}`              | `DeleteCollection`              |
| `POST /api/v1/collections/{name}/points`         | `UpsertPoints`                  |
| `DELETE /api/v1/collections/{name}/points`       | `DeletePoints`                  |
| `GET  /api/v1/collections/{name}/points/{id}`    | `GetPoint`                      |
| `GET  /api/v1/collections/{name}/points`         | `ListPoints`                    |
| `POST /api/v1/collections/{name}/search`         | `Search`                        |
| `POST /api/v1/collections/{name}/search/hybrid`  | `HybridSearch`                  |
| `POST /api/v1/collections/{name}/search/explain` | `ExplainSearch`                 |
| WebSocket streaming                              | `StreamUpsert` / `StreamSearch` |

### Diferenças em relação à API REST

- **Metadata**: no gRPC, metadata é transmitido como JSON string no campo `metadata_json` (em vez de objeto JSON inline).
- **Filtros**: filtros são JSON strings no campo `filter_json`.
- **Distance Metric**: enum protobuf `DistanceMetric` (1=Cosine, 2=DotProduct, 3=Euclidean).
- **Autenticação**: a API gRPC não inclui middleware de autenticação por API key (ideal para redes internas/service mesh). Para ambientes expostos, use um proxy com mTLS.

### Exemplos com `grpcurl`

**Criar coleção:**

```bash
grpcurl -plaintext -d '{
  "name": "embeddings",
  "dimension": 384,
  "distance": 1
}' localhost:50051 ferresdb.v1.FerresDB/CreateCollection
```

**Listar coleções:**

```bash
grpcurl -plaintext localhost:50051 ferresdb.v1.FerresDB/ListCollections
```

**Upsert de pontos:**

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

**Busca vetorial:**

```bash
grpcurl -plaintext -d '{
  "collection": "embeddings",
  "vector": [0.1, 0.2, 0.3],
  "limit": 5
}' localhost:50051 ferresdb.v1.FerresDB/Search
```

**Busca híbrida:**

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

### Gerando clientes gRPC

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
