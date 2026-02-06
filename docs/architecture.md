# Arquitetura — Visão Geral

Documento de entrada para a arquitetura do FerresDB Core. Para detalhes de componentes, fluxos e decisões, veja [architecture.md](architecture.md).

## Diagrama de Componentes

```mermaid
flowchart TB
  subgraph clients [Clientes]
    CLI[CLI]
    RAG[RAG / Ingestão]
    HTTP[Clientes HTTP]
  end

  subgraph server [Servidor HTTP - crates/server]
    API[REST API]
    Handlers[Handlers]
    State[AppState]
    API --> Handlers
    Handlers --> State
  end

  subgraph core [FerresDB Core - crates/core]
    VectorDB[VectorDB]
    Coll[Collection]
    Point[Point]
    ANN[ANNIndex / HNSW]
    Storage[Storage]
    WAL[WAL]
    VectorDB --> Coll
    Coll --> Point
    Coll --> ANN
    VectorDB --> Storage
    VectorDB --> WAL
  end

  subgraph sdk [SDK - crates/sdk-rust]
    SDK[ferres-db-sdk]
  end

  CLI --> API
  RAG --> API
  HTTP --> API
  SDK --> API
  State --> VectorDB
```

## Camadas

| Camada   | Crates     | Responsabilidade                         |
| -------- | ---------- | ---------------------------------------- |
| **API**  | `server`   | REST (Axum), handlers, métricas, health  |
| **Core** | `core`     | VectorDB, Collection, HNSW, Storage, WAL |
| **SDK**  | `sdk-rust` | Cliente Rust (HTTP, busca híbrida)       |

## Fluxo High-Level

```mermaid
sequenceDiagram
  participant C as Cliente
  participant API as REST API
  participant DB as VectorDB
  participant Col as Collection
  participant HNSW as HNSW
  participant Disk as Storage/WAL

  C->>API: POST /collections/:name/points
  API->>DB: upsert_points()
  DB->>Disk: WAL append
  DB->>Col: insert()
  Col->>HNSW: add_point()
  DB->>Disk: save (snapshot se threshold)
  API-->>C: 200 OK

  C->>API: POST /collections/:name/search
  API->>DB: search()
  DB->>Col: search()
  Col->>HNSW: search()
  HNSW-->>Col: ids + scores
  Col-->>DB: SearchResult[]
  API-->>C: JSON results
```

## Documentação Detalhada

- **[architecture.md](architecture.md)** — Componentes (Point, Collection, ANNIndex, Storage), fluxos (Insert, Search, Delete, Load), decisões de design, thread-safety, performance e extensibilidade.
- **[api.md](api.md)** — Referência dos endpoints REST.
- **[ADR/](ADR/)** — Architecture Decision Records (decisões em arquivos numerados).

## Estrutura do Repositório

```
ferres-db-core/
├── crates/
│   ├── core/       # Motor: VectorDB, Collection, HNSW, Storage, WAL, BM25
│   ├── server/     # Servidor HTTP (Axum), handlers, métricas
│   └── sdk-rust/   # SDK Rust (cliente HTTP)
├── docs/
│   ├── ARCHITECTURE.md   # Este arquivo (visão + diagramas)
│   ├── architecture.md  # Documentação detalhada
│   ├── ADR/              # Decision records
│   ├── api.md
│   └── sdk.md
├── examples/       # Ingestão, simple_rag
├── tests/          # E2E, fixtures
└── CONTRIBUTING.md # Guia de contribuição
```
