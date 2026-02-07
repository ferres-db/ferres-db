# Changelog

Alterações notáveis do projeto, agrupadas por semana. O formato é baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.0.0/).

## [Unreleased]

### Added

- **Query Cost Estimation — estimativa de custo de queries antes da execução**
  - Novo módulo `crates/core/src/cost.rs` com tipos: `QueryCostEstimate`, `CostBreakdown`, `CostEstimateParams`.
  - Função `estimate_search_cost` com heurísticas baseadas em HNSW (O(log n × ef_search × dimension)), custo de filtro pós-busca, hidratação e overhead de rede.
  - Novo método `VectorDB::estimate_query_cost` — estima custo sem executar busca real.
  - Novo endpoint `POST /api/v1/collections/{name}/search/estimate` — retorna estimativa de latência, memória, nós visitados, flag `is_expensive` e recomendações de otimização.
  - Campo opcional `include_history` para incluir percentis históricos (p50/p95/p99) no response.
  - **Budget-Based Queries**: campo opcional `budget_ms` no endpoint `POST /search`. Se a estimativa exceder o orçamento, retorna `422 Unprocessable Entity` com a estimativa detalhada no body (sem executar a busca).
  - Novo erro `BudgetExceeded` (422) no server com estimativa de custo no body.
  - Recomendações automáticas: limit alto, filtros em coleções grandes, vetores de alta dimensão, ef_search alto.
  - Testes unitários em `cost.rs` (15 testes: valores positivos, filtro, tamanho, limit, is_expensive, recomendações, serialização, etc.).
  - Testes E2E: endpoint estimate, collection not found, budget rejeitado, budget aceito.
  - Documentação do endpoint e budget_ms em `docs/api.md`.

- **Explain Query — explicação detalhada de resultados de busca vetorial**
  - Novo módulo `crates/core/src/explain.rs` com tipos: `SearchExplanation`, `ExplainResult`, `FilterExplanation`, `ConditionResult`, `ExplainMeta`, `IndexStats`.
  - Novo método `search_explain` no trait `ANNIndex` (com implementação padrão e override otimizado em `HnswIndex`).
  - Novo método `search_explain` em `Collection` (delega ao índice).
  - Novo método `VectorDB::search_explain` — busca vetorial com explicação completa: score breakdown, avaliação de filtros condição-a-condição, ranking antes/depois de filtros e estatísticas do índice.
  - Novo endpoint `POST /api/v1/collections/{name}/search/explain` no servidor HTTP.
  - Helper `evaluate_condition` para avaliar individualmente cada condição de filtro contra metadata.
  - Testes unitários para explain (basic, com filtro, com BM25 habilitado, avaliação de condições).
  - Documentação do endpoint em `docs/api.md`.

- **OpenTelemetry Distributed Tracing — tracing distribuído completo de cada busca vetorial**
  - Aprimorado `crates/server/src/tracing_otel.rs`: leitura explícita de `OTEL_EXPORTER_OTLP_ENDPOINT` (padrão `http://localhost:4317`), registro do propagador global W3C Trace Context (`traceparent`/`tracestate`).
  - Spans enriquecidos em `search_points` e `search_hybrid` com 9+ atributos OTel: `db.collection`, `db.operation`, `db.vector.dimension`, `db.vector.limit`, `db.results.count`, `db.duration.search_ms`, `db.duration.hydrate_ms`, `db.index.type`, `db.index.ef_search`, `db.hybrid.alpha`. Valores preenchidos via `Span::current().record()`.
  - Spans filhos granulares no core: `collection.search` (atributos: `points`, `dimension`) em `Collection::search()` e `hnsw.search` (atributos: `candidates`, `ef`, `tombstones`) em `HnswIndex::search()`.
  - Propagação de trace context W3C no middleware `request_logger`: extração de `traceparent`/`tracestate` dos headers HTTP, linkagem ao span `http_request`, e inclusão de header `x-trace-id` na resposta para debugging.
  - Todo código OTel protegido por `#[cfg(feature = "otel")]` — servidor funciona normalmente sem a feature.
  - Seção "Observability" no `README.md`: como habilitar, variáveis de ambiente, exemplo com Jaeger, hierarquia de spans, tabela de atributos OTel.

### Fixed

- **Performance: liberação antecipada de read lock nos handlers de busca**
  - `search_points`: o `RwLockReadGuard` e o `DashMap Ref` agora são dropados imediatamente após a hidratação dos resultados (`collection.get()`), antes de filtro de metadata, construção de `QueryProfile`, inserção no `DashMap`, métricas Prometheus, `query_stats` e spawn do `query_logger`.
  - `search_hybrid`: mesmo padrão — lock liberado logo após hidratar os resultados do hybrid search (vetorial + BM25).
  - Impacto: reduz contenção do `RwLock`, permitindo que escritas concorrentes (upsert/delete) não fiquem bloqueadas por operações que não precisam do lock (métricas, logging, profiling).

---

## 2025-W05 (semana de 03–09 fev 2025)

### Added

- **API REST (servidor HTTP)**
  - `GET /health` — health check
  - `GET /metrics` — métricas Prometheus
  - `POST /api/v1/save` — persistir todas as coleções no disco
  - `POST /api/v1/collections` — criar coleção (name, dimension, distance, enable_bm25, bm25_text_field)
  - `GET /api/v1/collections` — listar coleções
  - `GET /api/v1/collections/{name}` — detalhes da coleção
  - `DELETE /api/v1/collections/{name}` — remover coleção
  - `POST /api/v1/collections/{name}/points` — upsert de pontos (até 1000 por request)
  - `DELETE /api/v1/collections/{name}/points` — remover pontos por IDs
  - `GET /api/v1/collections/{name}/points/{id}` — obter ponto por ID
  - `POST /api/v1/collections/{name}/search` — busca vetorial (vector, limit, filter)
  - `POST /api/v1/collections/{name}/search/hybrid` — busca híbrida (vetorial + BM25, RRF)
  - `GET /api/v1/collections/{name}/stats` — estatísticas (num_points, num_queries, latências)
- **Core**
  - Busca híbrida (vetorial + BM25) com RRF; suporte a `enable_bm25` e `bm25_text_field` na criação da coleção
  - Filtro por metadata na busca vetorial (igualdade)
- **SDK Rust (ferres-db-sdk)**
  - `FerresDbClient::new(base_url)` e `hybrid_search(collection, query_text, query_vector, limit, alpha)`
  - Tipos `HybridSearchResponse`, `SearchResultItem`, `SdkError`
- **Documentação**
  - [docs/api.md](docs/api.md) — referência da API HTTP com curl e schemas JSON
  - [docs/sdk.md](docs/sdk.md) — guia SDK Rust e uso da API em Python/TypeScript
  - README raiz: overview, diagrama de arquitetura (Mermaid), quick start em 3 passos, links para docs
  - [examples/simple_rag/README.md](examples/simple_rag/README.md) — tutorial passo a passo, troubleshooting, próximos passos
  - [CHANGELOG.md](CHANGELOG.md) — log de mudanças por semana

### Changed

- N/A (entrada inicial)

### Fixed

- N/A (entrada inicial)

---

## Como usar este changelog

- **Unreleased**: itens já implementados mas ainda não publicados em release.
- **Por semana**: use o formato `## YYYY-Wxx (semana de DD–DD mês YYYY)` ou `## DD/MM/YYYY - DD/MM/YYYY` e agrupe as mudanças em **Added**, **Changed**, **Fixed** (e opcionalmente **API**, **Deprecated**, **Removed**, **Security**).
