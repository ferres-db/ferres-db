# Architecture — Overview

Entry document for the FerresDB Core architecture. For component details, flows and decisions, see [architecture.md](architecture.md).

## Component Diagram

```mermaid
flowchart TB
  subgraph clients [Clients]
    CLI[CLI]
    RAG[RAG / Ingestion]
    HTTP[HTTP Clients]
  end

  subgraph server [HTTP Server - crates/server]
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

## Layers

| Layer    | Crates     | Responsibility                            |
| -------- | ---------- | ----------------------------------------- |
| **API**  | `server`   | REST (Axum), handlers, metrics, health    |
| **Core** | `core`     | VectorDB, Collection, HNSW, Storage, WAL  |
| **SDK**  | `sdk-rust` | Rust client (HTTP, hybrid search)         |

## High-Level Flow

```mermaid
sequenceDiagram
  participant C as Client
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
  DB->>Disk: save (snapshot if threshold)
  API-->>C: 200 OK

  C->>API: POST /collections/:name/search
  API->>DB: search()
  DB->>Col: search()
  Col->>HNSW: search()
  HNSW-->>Col: ids + scores
  Col-->>DB: SearchResult[]
  API-->>C: JSON results
```

## Detailed Documentation

- **[architecture.md](architecture.md)** — Components (Point, Collection, ANNIndex, Storage), flows (Insert, Search, Delete, Load), design decisions, thread-safety, performance and extensibility.
- **[api.md](api.md)** — REST endpoint reference.
- **[ADR/](ADR/)** — Architecture Decision Records (decisions in numbered files).

## Repository Structure

```
ferres-db-core/
├── crates/
│   ├── core/       # Engine: VectorDB, Collection, HNSW, Storage, WAL, BM25
│   ├── server/     # HTTP server (Axum), handlers, metrics
│   └── sdk-rust/   # Rust SDK (HTTP client)
├── docs/
│   ├── ARCHITECTURE.md   # This file (overview + diagrams)
│   ├── architecture.md  # Detailed documentation
│   ├── ADR/              # Decision records
│   ├── api.md
│   └── sdk.md
├── examples/       # Ingestion, simple_rag
├── tests/          # E2E, fixtures
└── CONTRIBUTING.md # Contribution guide
```
