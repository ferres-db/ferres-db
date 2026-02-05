# Changelog

Alterações notáveis do projeto, agrupadas por semana. O formato é baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.0.0/).

## [Unreleased]

- Alterações ainda não liberadas; serão listadas na próxima semana.

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
