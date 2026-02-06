# Architecture Decision Records (ADRs)

Este diretório contém as decisões arquiteturais do FerresDB Core no formato [ADR](https://adr.github.io/). Cada arquivo documenta uma decisão com contexto, alternativas e consequências.

## Índice

| ADR                                     | Título                                       | Status |
| --------------------------------------- | -------------------------------------------- | ------ |
| [0001](0001-hnsw-library.md)            | Uso de HNSW em vez de implementação própria  | Aceito |
| [0002](0002-jsonlines-persistence.md)   | Formato JSON-lines para persistência         | Aceito |
| [0003](0003-trait-object-ann-index.md)  | Trait Object para abstração de índice ANN    | Aceito |
| [0004](0004-tombstones-removal.md)      | Tombstones em vez de remoção real do HNSW    | Aceito |
| [0005](0005-l2-normalization-cosine.md) | Normalização L2 para métrica Cosine          | Aceito |
| [0006](0006-lru-cache-optional.md)      | Cache LRU opcional para resultados de busca  | Aceito |
| [0007](0007-rayon-parallelization.md)   | Paralelização com Rayon para batches grandes | Aceito |
| [0008](0008-vector-validation.md)       | Validação rigorosa de vetores                | Aceito |
| [0009](0009-atomic-writes.md)           | Atomic writes com temp-file + rename         | Aceito |
| [0010](0010-checksum-md5.md)            | Checksum MD5 para validação de integridade   | Aceito |

## Decisões propostas (rascunho)

- **WAL (Write-ahead log)** — Melhor durabilidade e throughput (já implementado; ADR formal pode ser adicionado).
- **Quantização de vetores** — Redução de memória (futuro).

## Referências

- [Architecture Decision Records](https://adr.github.io/)
- [Documenting Architecture Decisions](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) (Michael Nygard)
