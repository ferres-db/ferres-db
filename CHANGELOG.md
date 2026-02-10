# Changelog

Alterações notáveis do projeto, agrupadas por semana. O formato é baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.0.0/).

## [Unreleased]

### Added

- **Feature: Integrated native Cross-Encoder re-ranking via ONNX Runtime.** — Core: suporte opcional à crate `ort` (feature `rerank`) para carregar modelos Cross-Encoder (ex.: BGE-Reranker). Novo método `search_with_rerank`: recupera `limit * 5` candidatos via HNSW, re-pontua com o Cross-Encoder e retorna os top `limit` reordenados. API: parâmetro `rerank: bool` no body de `POST /api/v1/collections/{name}/search`; resposta inclui `rerank_ms` quando aplicável. Dashboard (Analytics): métrica "Re-ranking Overhead (ms)". Documentação em `docs/api.md`.

- **Security: Enhanced RBAC with namespace-level access control.** — API keys can be restricted to one or more namespaces (multitenancy). New `NamespaceAllowance` in the permissions model; API key store supports `allowed_namespaces` (create and update via `PUT /api/v1/keys/:id`). Middleware validates namespace from query param `namespace` or header `X-Namespace`; handlers validate namespace from request body (search, upsert, delete points). Dashboard: "Users/API Keys" allows assigning namespaces when creating a key and editing namespaces per key. Documented in `docs/api.md`.

- **Optimization: Dynamic HNSW auto-tuning based on real-time performance metrics.** — O índice HNSW passa a ajustar `ef_search` dinamicamente (FerresEngine): se a latência P95 estiver baixa e o recall for prioridade, o valor é aumentado; se a latência estiver alta (proxy para CPU sob estresse), é reduzido. A lógica de auto-tune está em `collection.rs` (`apply_hnsw_auto_tune`); o servidor executa um ciclo a cada 60s usando P95 do `query_stats` por coleção. Novos campos em `GET /api/v1/stats/global`: `hnsw_auto_tune_enabled`, `index_optimization_label` ("Optimized by FerresEngine"); em `GET /api/v1/collections/{name}/stats`: `ef_search_current`, `hnsw_auto_tune_enabled`. Dashboard (Overview): badge e card "Index" com "Optimized by FerresEngine". Documentação em `docs/api.md`.

- **Feature: Added Point-in-Time Recovery (PITR) support using timestamped WAL.** — Cada entrada do WAL já inclui timestamp Unix; ao gravar um snapshot, o servidor persiste `last_snapshot_timestamp` no diretório da coleção. Novo endpoint `POST /api/v1/admin/restore` (Admin) aceita `{ "timestamp": <unix_sec>, "collection": "<name>?" }` e restaura uma ou todas as coleções ao estado nesse momento (carrega o último snapshot e reaplica o WAL apenas até o timestamp). `GET /api/v1/admin/restore/points` lista pontos de restauração (snapshot + timestamps do WAL) por coleção. Dashboard: nova aba **Snapshots & Recovery** para visualizar pontos de restauração e acionar PITR. Documentação em `docs/api.md` (guia de recuperação de desastres).

- **Security: Added secure S3 configuration management and expanded audit logging for admin actions.** — Dashboard Settings: section to configure S3 backup (Bucket, Region, Endpoint); credential fields (Secret Key) are hidden by default (password inputs). New endpoint `POST /api/v1/admin/settings/test-s3` validates S3 connection before saving. All "Backup to S3" and "Reindex" operations triggered via Dashboard (or API) are recorded in the audit log.

- **Optimizations: Added automatic cache warmup on startup.** — Ao iniciar, o servidor lê as últimas 50 queries do `query_logger` (queries.log), reexecuta-as em background para carregar os índices HNSW na RAM (Hot Tier) e popular o `search_cache`. Logs de tracing indicam o progresso do warmup (`warmup: starting cache warmup`, `warmup: ran query`, `warmup: cache warmup completed`). O log de queries passou a gravar o vetor completo (opcional) para permitir o replay.

- **Physical storage isolation for improved multitenancy security** — Com opção `namespace_physical_isolation` (core: `StorageOptions`, servidor: `namespace_physical_isolation` no config.toml ou `FERRESDB_NAMESPACE_PHYSICAL_ISOLATION`), os pontos de cada namespace passam a ser gravados em `data/collections/<name>/namespaces/<namespace>/points.bin` (e opcionalmente `index.bin`). O VectorDB carrega índices de forma independente por namespace quando esses diretórios existem, permitindo snapshot por namespace e limpeza física de dados de um tenant sem afetar outros. Documentação em `docs/api.md`.

- **Backup S3** — Integração com AWS S3 para backups: novo endpoint `POST /api/v1/admin/backup` (apenas Admin) gera um snapshot binário (tar.gz) do diretório de storage e faz upload para um bucket S3 configurável. Configuração via `config.toml` ou variáveis de ambiente: **Region** (`s3_region` / `FERRESDB_S3_REGION` ou `AWS_REGION`), **Bucket** (`s3_bucket` / `FERRESDB_S3_BUCKET`), **Credentials** (`s3_access_key_id` / `s3_secret_access_key` ou `FERRESDB_S3_ACCESS_KEY_ID` / `FERRESDB_S3_SECRET_ACCESS_KEY`, ou `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`). Dependências no servidor: `aws-sdk-s3`, `aws-config`, `tar`, `flate2`. Dashboard: nova página **Definições** com botão "Export to Cloud" (visível apenas para Admin). Documentação em `docs/api.md`.

- **Replication (Experimental)** — Base para Read Replicas: no core, `Wal::stream_from(collection_dir, position)` para leitura incremental do WAL; servidor com `--replica-of <ADDR>` ou `FERRESDB_REPLICA_OF` inicia como réplica; endpoints de escrita (POST/PUT/DELETE em coleções, pontos, save, reindex, etc.) retornam **405 Method Not Allowed** em réplicas; worker (feature `grpc`) consome WAL do líder via gRPC `StreamWal` e aplica no VectorDB local; dashboard exibe "Role: Leader" ou "Role: Replica" no Overview; `GET /api/v1/stats/global` inclui campo `role`. Documentação em `docs/api.md` (seção Replication).

## [0.1.0-STABLE] - 09/02/2026

Primeira versão estável do FerresDB, com polimento final de performance, analytics e documentação.

### Added

- **Performance: SIMD (AVX2/SSE4.1)** — Kernels de distância em `crates/core/src/search.rs` com a crate `pulp`: `euclidean_distance` e `dot_product` com despacho em runtime (AVX2 8× f32, SSE4.1 4× f32) e fallback escalar. Distância assimétrica SQ8 (f32×u8) em `quantization.rs` processa múltiplos bytes em paralelo. Servidor registra no startup via `tracing`: "SIMD acceleration: active" ou "scalar fallback". Dashboard (Overview): Badge "SIMD: Active" (verde) ou "SIMD: Scalar" (amarelo) a partir de `GET /api/v1/stats/global` (`simd_enabled`).

- **Analytics: correção e visibilidade** — O endpoint `GET /api/v1/stats/analytics` passa a preencher `time_series_10m` com dados reais: leitura fresca do `queries.log` (sem depender do cache de 1h) para `throughput_per_minute`, `recent_latencies` e `p95_latency_ms`. Query logger faz `flush` após cada escrita para que o analytics leia dados imediatamente. Dashboard: gráfico de ingestão (pontos/min), área de latência e histograma P95 (distribuição por faixas de ms); card "Cache Hit Rate %" nos KPIs.

- **Documentação** — `docs/api.md`: especificação dos campos de série temporal (`time_series_10m`) e da flag `simd_enabled` (stats/global). CHANGELOG e exemplos do SDK alinhados à estrutura final de inserção e busca.

### Fixed

- **Analytics: dados de queries no Dashboard** — O buffer de logs não era lido de forma atualizada pelo endpoint de analytics (cache 1h). Passou a usar `entries_10m_fresh()` e `p95_latency_10m_fresh()` para leitura direta do arquivo na construção de `time_series_10m`, garantindo que os gráficos do Dashboard exibam latência e throughput reais.

---

## [Released] - 09/02/2026

### Added

- **Feature: Embedded Model Context Protocol (MCP) support via STDIO.** — Servidor MCP embutido no binário do FerresDB; ativação com a flag `--mcp` ou a variável de ambiente `FERRESDB_ENABLE_MCP=true`. Ferramentas expostas: `search_points` (busca vetorial com pre-filtering nativo), `upsert_points` e `get_stats`. O protocolo usa stdin/stdout; os logs do servidor são redirecionados para stderr quando o modo MCP está ativo. Requer build com a feature `mcp` (`cargo build -p ferres-db-server --features mcp`). Documentação em `docs/api.md` (seção Model Context Protocol) e `README.md` (conexão com Claude Desktop).

- **Dashboard: Added real-time ingestion throughput and latency charts.** — Página Analytics passa a exibir gráfico de linha (Recharts) para ingestão (throughput, pontos/min nos últimos 10 min) e gráfico de área para latência de busca (ms) nas últimas consultas; KPIs incluem "Cache Hit Rate %" baseado no `search_cache` do core. Backend: `query_log_analytics` com `entries_10m()`, `p95_latency_10m()` e `avg_points_per_second_10m()`; buffer de ingestão em `AppState` para séries temporais; endpoint `GET /api/v1/stats/analytics` estendido com `time_series_10m` e `cache_hit_rate_pct`. Documentação em `docs/api.md`.

- **Feature: Support for named multi-vector points per document.** — Cada ponto pode ter um vetor principal (`vector`) e opcionalmente múltiplos vetores nomeados (`vectors: HashMap<String, Vec<f32>>`), por exemplo `title_vector` e `content_vector`. A busca aceita o parâmetro `vector_field` para consultar contra o vetor principal (`default`) ou contra um campo nomeado. Índices ANN separados são mantidos por campo vetorial; inserção, remoção e persistência (JSONL/bincode) suportam a nova estrutura. Documentação em `docs/api.md`.

- **Storage: Added Zstd compression for WAL and binary snapshot support.** — WAL pode usar compressão Zstd opcional (menor uso de disco em `wal.log`); snapshots de pontos podem ser gravados em `points.bin` (bincode) em vez de `points.jsonl`, reduzindo tamanho e tempo de carregamento. Configuração via `StorageOptions` (core), `wal_compression` / `binary_snapshot` no servidor (config.toml ou env `FERRESDB_WAL_COMPRESSION`, `FERRESDB_BINARY_SNAPSHOT`). Documentação em `docs/api.md`.

- **Ecossystem: Added foundation for LangChain and LlamaIndex integrations.**

- **Search: Implemented native HNSW pre-filtering for higher accuracy with metadata.** — O filtro é aplicado durante a exploração do grafo (nós que não satisfazem o predicado são ignorados antes de entrar na lista de candidatos). A busca continua explorando com `ef` crescente até obter até `limit` resultados válidos ou exaurir o grafo.

- **Performance: SIMD kernels implemented with hardware status visibility in Dashboard.** — Kernels SIMD (AVX2/SSE4.1) para `euclidean_distance` e `dot_product` em `crates/core/src/search.rs` (foco em `QuantizedHnswIndex` SQ8); detecção em runtime via `simd_enabled()`. Endpoint `GET /api/v1/stats/global` expõe `simd_enabled: bool`; Dashboard (Overview) exibe Badge "SIMD Acceleration: Active" ou "SIMD: Scalar Fallback".

- **Performance: Added SIMD-accelerated distance kernels (AVX2/SSE).** — Kernels de distância (`euclidean_distance`, `dot_product`) em `crates/core/src/search.rs` usam a crate `pulp` para abstração SIMD segura, com despacho em tempo de execução para AVX2 (8× f32) ou SSE4.1 (4× f32) e fallback escalar automático. Distância assimétrica SQ8 (f32×u8) permanece otimizada em `quantization.rs` (múltiplos bytes em paralelo).

- **Features: Time-to-Live (TTL) support for automatic data expiration**

- **Support for Logical Namespaces (Multitenancy)** — Isolamento de dados por cliente na mesma coleção física via campo opcional `namespace` em Point. Permite evitar milhares de coleções: vários tenants compartilham uma coleção e os dados são filtrados por namespace. Inclui: campo `namespace` em Point (opcional); `MetadataFilter` com condição de primeira classe `$namespace` e métodos `matches_namespace`/`matches_point`; chave interna composta `(namespace, id)` para unicidade; persistência em storage e WAL; parâmetro `namespace` em buscas, get point e delete. Documentação em `docs/api.md`.

- **Auto-Reindex em background (worker)** — Worker em background que a cada 30 minutos percorre as coleções e verifica o rácio de tombstones (`tombstone_count / total_indexed`). Quando o rácio excede 20%, dispara reindex automático usando a lógica de swap de índice existente. Logs detalhados de início e fim de ciclo e de cada compactação via `tracing`. Em `crates/core`: `tombstone_ratio()`, `total_indexed_len()` em `Collection`, documentação de `total_indexed` em `needs_reindex`. Em `crates/server`: task Tokio em `main.rs`, `run_auto_reindex_cycle()` em `handlers/reindex.rs`, graceful shutdown da task.

### Performance / Search

- **Optimizations: SIMD-accelerated distance kernels** — Cálculos de distância vetorial acelerados com instruções SIMD (AVX2 e SSE4.1) e detecção em tempo de execução com fallback escalar. Distâncias f32×f32: `euclidean_distance` e `dot_product` em `search.rs` com kernels AVX2 (8× f32) e SSE4.1 (4× f32); distância assimétrica f32×u8 (SQ8): `asymmetric_distance` em `quantization.rs` acelerada para re-ranking do `QuantizedHnswIndex`. Ganho de throughput significativo em vetores de 256–384 dimensões em CPUs com AVX2.

- **Native HNSW pre-filtering** — A busca com filtro de metadata passou a aplicar o filtro **durante** a exploração do grafo HNSW (via `search_filter` do hnsw_rs), em vez de buscar `limit*10` resultados e filtrar depois. Garante maior precisão e consistência no número de resultados retornados (até `limit` que satisfazem o filtro). O trait `ANNIndex` foi estendido com parâmetro opcional `predicate` em `search` e `search_explain`; `HnswIndex` e `QuantizedHnswIndex` utilizam pre-filtering nativo quando o predicado está presente.

## [Released] - 08/02/2026 - 12:00

### Added

- **API gRPC nativa — alternativa de alta performance à API REST com streaming bidirecional**
  - Feature flag `grpc` no `crates/server/Cargo.toml` — servidor funciona sem gRPC por padrão (só REST).
  - Proto file `crates/server/proto/ferresdb.proto` com package `ferresdb.v1`.
  - Serviço `FerresDB` com 13 RPCs espelhando a API REST:
    - `CreateCollection`, `GetCollection`, `ListCollections`, `DeleteCollection` — CRUD de coleções.
    - `UpsertPoints`, `DeletePoints`, `GetPoint`, `ListPoints` — gerenciamento de pontos.
    - `Search`, `HybridSearch`, `ExplainSearch` — busca vetorial, híbrida e explain.
    - `StreamUpsert` (client→server streaming) e `StreamSearch` (bidirecional) — operações em streaming.
  - Novo módulo `crates/server/src/grpc.rs` (~960 linhas): implementação completa do serviço gRPC reutilizando `AppState`, `Collection`, `Point`, `MetadataFilter` — zero duplicação de lógica de negócio.
  - `build.rs` com `tonic-build` para compilação automática do proto (requer `protoc`).
  - Server gRPC (tonic) escuta na porta 50051 (configurável via `GRPC_PORT` env) em paralelo com REST.
  - Dependências opcionais: `tonic 0.12`, `prost 0.13`, `tonic-build 0.12`, `async-stream 0.3`.
  - Metadata e filtros transmitidos como JSON string (`metadata_json`, `filter_json`) no gRPC.
  - `DistanceMetric` mapeado para enum protobuf (1=Cosine, 2=DotProduct, 3=Euclidean).
  - Métricas Prometheus e query stats registrados para queries gRPC (mesmos counters/histograms do REST).
  - Documentação: `docs/api.md` com seção gRPC completa (mapeamento REST→gRPC, exemplos `grpcurl`, geração de clientes).
  - SDKs: READMEs atualizados com instruções para gerar stubs gRPC em Python, TypeScript e Go.

- **Background Reindex — reconstrução de índice ANN sem downtime**
  - Novo módulo `crates/core/src/reindex.rs` com toda a lógica de reindex em background.
  - `ReindexJob`, `ReindexStatus`, `ReindexStats`: tipos para rastrear jobs de reindex.
  - Fluxo de 3 fases: **Building** (thread separada, buscas continuam no índice antigo), **Swapping** (write lock < 1ms para trocar índices), **Cleanup** (drop do índice antigo).
  - `build_new_index()`: constrói novo `Box<dyn ANNIndex>` a partir de snapshot de pontos — sem tombstones.
  - `apply_delta()`: reconcilia inserções/remoções que ocorreram durante a fase de build.
  - `needs_reindex()`: detecta quando tombstones > 20% dos pontos indexados.
  - `estimate_index_size()`: estima tamanho do índice em bytes.
  - Trait `ANNIndex` estendido com `tombstone_count()` (implementado em `HnswIndex` e `QuantizedHnswIndex`).
  - `Collection` estendido com `tombstone_count()`, `points_snapshot()`, `swap_index()`.
  - Novos endpoints no server:
    - `POST /api/v1/collections/{name}/reindex` — inicia job de reindex (retorna 202 Accepted).
    - `GET /api/v1/collections/{name}/reindex/{job_id}` — status do job.
    - `GET /api/v1/collections/{name}/reindex` — lista jobs da collection.
  - Auto-reindex: após deleção de pontos, se tombstones > 20%, um reindex é disparado automaticamente.
  - Jobs registrados no `AppState` via `DashMap<String, Arc<RwLock<ReindexJob>>>`.
  - Restrição: apenas 1 reindex por collection por vez (retorna 409 se já existe job ativo).
  - Testes: `test_reindex_cleans_tombstones`, `test_reindex_concurrent_search`, `test_reindex_with_concurrent_writes`, `test_reindex_job_lifecycle`, `test_reindex_job_failure`, `test_build_new_index`, `test_apply_delta_additions`, `test_apply_delta_removals`, `test_needs_reindex`, `test_estimate_index_size`, `test_reindex_stats_default`, `test_reindex_job_serialization`.
  - SDKs atualizados:
    - **Python**: `start_reindex()`, `get_reindex_job()`, `list_reindex_jobs()` + modelos `ReindexJob`, `ReindexStatus`, `ReindexStats`, `StartReindexResponse`.
    - **TypeScript**: `startReindex()`, `getReindexJob()`, `listReindexJobs()` + tipos e schemas Zod correspondentes.
  - Dashboard: API client com `reindexApi.start()`, `reindexApi.getJob()`, `reindexApi.listJobs()`.
  - Documentação: `docs/api.md` atualizado com endpoints, schemas e exemplos.

- **Fusion Strategies para Hybrid Search — Reciprocal Rank Fusion (RRF) como alternativa ao weighted score**
  - Novo módulo `crates/core/src/fusion.rs` com algoritmos de fusão desacoplados.
  - `FusionStrategy` (enum): `WeightedScore { alpha }` (compatível com comportamento original) e `RRF { k }` (fusão pura por rank).
  - `reciprocal_rank_fusion()`: fusão genérica de N rankings via `score = Σ 1/(k + rank_i)`. Suporta qualquer número de rankers.
  - `weighted_fusion()`: fusão ponderada de 2 rankings (vetorial + keyword) com alpha. Replica o comportamento original.
  - `Collection::hybrid_search()` agora aceita `FusionStrategy` em vez de `alpha` diretamente.
  - Novos campos opcionais no endpoint `POST /api/v1/collections/{name}/search/hybrid`:
    - `fusion`: `"weighted"` (default) ou `"rrf"`.
    - `rrf_k`: constante k para RRF (default: 60).
  - Backward compatible: requests sem `fusion` usam `"weighted"` com `alpha` (comportamento idêntico ao anterior).
  - Testes: `test_rrf_basic`, `test_rrf_no_overlap`, `test_rrf_vs_weighted`, `test_weighted_backward_compat`, `test_rrf_limit`, `test_rrf_empty_rankings`, `test_rrf_single_ranking`, `test_weighted_fusion_extreme_alpha`, `test_rrf_three_rankers`.
  - SDKs atualizados:
    - **Python**: parâmetros `fusion` e `rrf_k` em `hybrid_search()`.
    - **TypeScript**: campos `fusion` e `rrf_k` em `HybridSearchQuery`.
  - Dashboard: seletor de estratégia de fusão na aba Hybrid do Query Tester.
  - Documentação: `api.md` atualizado com novos parâmetros, exemplos e guia de quando usar RRF vs weighted.

- **Tiered Storage — movimentação automática de vetores entre camadas de armazenamento baseada em frequência de acesso**
  - Novo módulo `crates/core/src/tiered.rs` com toda a lógica de tiered storage.
  - `TieredStorageConfig`: configuração opt-in com thresholds para Hot/Warm/Cold e intervalo de compactação.
  - `StorageTier` (enum): `Hot` (RAM), `Warm` (mmap), `Cold` (disco on-demand).
  - `AccessTracker`: rastreio de último acesso e contagem por ponto para decisão automática de tier.
  - `WarmStorage`: armazenamento de vetores em memory-mapped files via `memmap2`.
  - `ColdStorage`: persistência completa de pontos em disco (JSON), carregados on-demand.
  - `TieredCollection`: wrapper sobre `Collection` que gerencia Hot/Warm/Cold com promoção e demoção automática.
  - Background compaction: task periódica que demove pontos Hot→Warm→Cold baseado em thresholds de acesso.
  - Promoção automática: qualquer acesso a ponto Warm/Cold promove para Hot.
  - Grafo HNSW **sempre** em memória — apenas dados dos pontos são tiered.
  - `CollectionConfig` estendido com campo `tiered_storage` (`#[serde(default)]` para backward compatibility).
  - `CollectionMeta` em `storage.rs` inclui `tiered_storage` para persistência.
  - `FileStorage::save_tier_metadata` / `load_tier_metadata` para persistir `TierMetadata` (tiers, acessos).
  - Novo endpoint `GET /api/v1/collections/{name}/tiers`: retorna distribuição de pontos por tier e memória estimada.
  - Testes: `test_tier_demotion`, `test_tier_promotion`, `test_search_across_tiers`, `test_compaction`, `test_tier_distribution`, `test_tiered_disabled_everything_hot`, `test_tier_metadata_serialization`, `bench_search_latency_hot_vs_cold`.
  - Dependência: `memmap2 = "0.9"` no workspace.
  - SDKs atualizados:
    - **Python**: `TieredStorageConfig` model, `get_tier_distribution()` no client, `TierDistribution` response model.
    - **TypeScript**: `TieredStorageConfig` interface/schema, `getTierDistribution()` no client, `TierDistribution` response type.
  - Documentação: `api.md` atualizado com novo endpoint e configuração de tiered storage.

## [Released] - 07/02/2026 - 15:00 - 0.2.0

### Added

- **Real-time Streaming via WebSocket — ingestão e subscrição de eventos em tempo real**
  - Novo endpoint `GET /api/v1/ws` para upgrade HTTP → WebSocket.
  - Protocolo JSON sobre WebSocket com mensagens tipadas:
    - `upsert`: ingestão de pontos em tempo real com batch automático (debounce 10ms).
    - `subscribe`: subscrição para eventos de uma coleção (`upsert`, `delete`).
    - `ping`/`pong`: heartbeat aplicacional.
    - `ack`: confirmação de operação com `upserted`, `failed`, `took_ms`.
    - `event`: notificação de mudanças (collection, action, point_ids, timestamp).
    - `error`: erro com mensagem e código HTTP.
  - Novo `handlers/streaming.rs` com `ws_handler` e `handle_ws_connection`.
  - Batch automático: acumula mensagens por 10ms antes de flush (debounce) para melhor throughput.
  - Heartbeat: ping a cada 30s, desconexão se pong não chegar em 10s.
  - Timeout de inatividade: 5 minutos sem atividade fecha a conexão.
  - Autenticação: aceita API key como query param (`?token=sk-xxx`) ou header `Authorization: Bearer <key>`.
  - Limite configurável de 100 conexões WebSocket simultâneas.
  - Mensagens > 10MB rejeitadas automaticamente.
  - Novo `CollectionEvent` e `event_channels` (broadcast) no `AppState` para propagação de eventos.
  - Handlers REST `upsert_points` e `delete_points` agora emitem eventos no broadcast channel para subscribers WebSocket.
  - Métricas Prometheus: `ws_connections_active` (gauge), `ws_messages_received_total` (counter por tipo), `ws_messages_sent_total` (counter por tipo).
  - Testes E2E: upsert de 10 pontos via WS, ping/pong, subscribe + evento via REST, rejeição sem auth, mensagem inválida, coleção inexistente.
  - Dependências: `axum` com feature `ws`, `tokio-tungstenite`.

- **Scalar Quantization (SQ8) — compressão de vetores f32 para u8 com ~4× economia de memória**
  - Novo módulo `crates/core/src/quantization.rs`: `QuantizationConfig` (enum `None | Scalar`), `ScalarQuantizationConfig` (dtype, always_ram, quantile), `ScalarType::Int8`, `ScalarQuantizationParams` (mins, maxs, scales por dimensão).
  - `ScalarQuantizationParams::calibrate()`: calibra min/max/scale por dimensão com percentis para robustez contra outliers. Amostra limitada a 10K vetores para performance.
  - `ScalarQuantizationParams::quantize()`: mapeia `f32` → `u8` por dimensão (`[min,max]` → `[0,255]`).
  - `ScalarQuantizationParams::dequantize()`: operação inversa (aproximada) para reconstrução.
  - `ScalarQuantizationParams::asymmetric_distance()`: distância assimétrica (query f32 vs candidato u8) para Euclidean, Cosine e DotProduct — preserva mais precisão que quantizar ambos.
  - Novo `QuantizedHnswIndex` em `search.rs`: implementa `ANNIndex` combinando HNSW (para navegação do grafo) com vetores quantizados u8.
    - `build()`: calibra params, quantiza vetores, constrói HNSW com vetores dequantizados.
    - `search()`: busca HNSW expandida → re-rank com distância assimétrica → re-rank opcional com originais f32 (se `always_ram=true`).
    - `add_point()`: quantiza vetor, insere no HNSW com dequantizado.
    - `remove_point()`: delega tombstone para HNSW interno.
  - Factory function `create_ann_index()`: seleciona `HnswIndex` ou `QuantizedHnswIndex` baseado na config.
  - `CollectionConfig` estendido com campo `quantization: QuantizationConfig` (`#[serde(default)]` para backward compatibility).
  - `Collection::new()` agora usa `create_ann_index()` para criar o índice adequado.
  - `CreateCollectionRequest` no server aceita campo `quantization` (opt-in via API).
  - `CollectionMeta` em `storage.rs` inclui `quantization` para persistência.
  - Benchmarks em `crates/core/benches/performance.rs`: recall@10 SQ8 vs f32, latência de busca, uso de memória para 10K e 100K vetores.
  - Testes: `test_sq8_calibration`, `test_sq8_roundtrip` (erro < 1%), `test_sq8_recall` (recall@10 > 90% vs f32), `test_sq8_memory` (4× compressão), `test_quantized_hnsw_basic`, `test_quantized_hnsw_always_ram`, `test_quantized_hnsw_add_point`, `test_quantized_hnsw_remove_point`, `test_create_ann_index_*`, testes de serialização/desserialização, testes com outliers.
  - Coleções existentes sem quantização continuam funcionando sem mudança (default: `QuantizationConfig::None`).

- **RBAC (Role-Based Access Control) granular com audit trail**
  - Novo módulo `crates/server/src/permissions.rs`: `Resource` (AllCollections, Collection(name)), `Action` (Read, Write, Create, Delete, Admin), `MetadataRestriction`, `Permission`, `PermissionResult`, `check_permission`, `merge_restriction_filter`.
  - Novo módulo `crates/server/src/audit.rs`: `AuditEntry`, `AuditResult`, `AuditLogger` (append-only JSONL, rotação diária `audit-YYYY-MM-DD.jsonl`), escrita assíncrona via `tokio::spawn`.
  - UserStore estendido: coluna `permissions TEXT` (JSON) em SQLite, migração `ALTER TABLE`, `get_permissions`, `update_permissions`, `create_with_permissions`; `UserInfo` com campo opcional `permissions`.
  - Auth: `AuthUser` com `permissions: Option<Vec<Permission>>`; extractor `AuthenticatedUser` carrega permissões do UserStore via state; helper `check_user_permission` (Admin bypassa, fallback legado por role).
  - Enforcement: handlers de points (search, search_hybrid, upsert, delete_points, explain_search, estimate_search), collections (create, delete), save, keys, users usam `AuthenticatedUser` e verificam permissão granular; em `search_points` a `MetadataRestriction` é injetada no filtro (AND com request).
  - Novo erro `ApiError::Forbidden` (403).
  - Endpoint `GET /api/v1/audit` (Admin only): query params `user`, `action`, `resource`, `from`, `to`, `limit`; retorna entradas de auditoria filtradas.
  - Endpoint `PUT /api/v1/users/{username}/permissions` para atualizar permissões granulares (Admin).
  - Instrumentação de todos os handlers com audit trail (login, search, upsert, delete_points, create/delete collection, save, create/delete API key, create/delete user, update password/permissions).
  - Documentação em `docs/api.md`: modelo RBAC, endpoints de permissões e audit, enforcement por endpoint.
  - Testes em `crates/server/tests/rbac_test.rs`: viewer não pode upsert, viewer pode search, admin bypassa restrições, MetadataRestriction filtra resultados, permissões por coleção, write granular, audit registra ações e negações, audit requer Admin; testes unitários em `permissions` e `audit`, persistência de permissões no UserStore.

## [Released] - 07/02/2026 - 11:00 - 0.1.1

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
