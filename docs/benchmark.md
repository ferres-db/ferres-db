# FerresDB — Benchmark e Stress Test

Ferramenta oficial de **benchmark** e **stress test** para o FerresDB. Permite avaliar throughput de ingestão, QPS e latência (P95/P99) de buscas vetoriais, e robustez sob carga mista (chaos).

## Pré-requisitos

- Servidor FerresDB em execução (por exemplo: `make run` ou `cargo run -p ferres-db-server`).
- URL padrão: `http://localhost:8080` (alterável com `--url`).
- Se o servidor exigir autenticação: **API key** do FerresDB (ver [docs/api.md](api.md) — header `Authorization: Bearer <api-key>`). Passe `--api-key <key>` ou defina `FERRESDB_API_KEY`.
- Para ingest/busca com **embeddings reais** (OpenAI): chave da API OpenAI em `--openai-api-key <key>` ou variável `OPENAI_API_KEY`. A ferramenta cria a coleção, gera textos, chama a API de embeddings da OpenAI e insere/busca com esses vetores.

## Instalação

A ferramenta é um binário do workspace:

```bash
cargo build --release -p ferres-bench
```

O executável fica em `target/release/ferres-bench` (ou `ferres-bench.exe` no Windows).

## Uso rápido (Makefile)

```bash
# Compila e roda um teste padrão (ingest 10k vetores + search 15s)
make bench

# Apenas ingest (10k vetores, dim 768, concorrência 20)
make bench-ingest

# Apenas search (15s, concorrência 50)
make bench-search

# Modo chaos (30s, writers + readers + criação de coleções)
make bench-chaos
```

## Modos de operação

### 1. Ingest (Write Stress)

Cria a coleção (se não existir), gera vetores e dispara inserts concorrentes. Com `--openai-api-key`, usa **embeddings OpenAI**: gera textos ("Document N..."), chama a API de embeddings e insere os vetores (dimensão vem do modelo, ex.: 1536 para `text-embedding-3-small`). Sem OpenAI, usa vetores aleatórios e `--dim` (ou default 1536).

```bash
# Com API key do FerresDB e embeddings OpenAI (recomendado para cenário realista)
ferres-bench ingest --api-key <ferres-api-key> --openai-api-key <openai-key> --vectors 10000 --concurrency 20

# Apenas vetores aleatórios (sem OpenAI)
ferres-bench ingest --vectors 100000 --dim 768 --concurrency 50
```

| Argumento           | Default                | Descrição                                                       |
| ------------------- | ---------------------- | --------------------------------------------------------------- |
| `--api-key`         | —                      | API key do FerresDB (env: `FERRESDB_API_KEY`). Ver docs/api.md. |
| `--vectors`         | 10000                  | Número total de vetores a inserir                               |
| `--dim`             | —                      | Dimensão (só sem OpenAI). Com OpenAI a dimensão vem do modelo.  |
| `--concurrency`     | 50                     | Número de tarefas concorrentes                                  |
| `--collection`      | bench                  | Nome da coleção (criada automaticamente se não existir)         |
| `--openai-api-key`  | —                      | Chave OpenAI para embeddings (env: `OPENAI_API_KEY`)            |
| `--embedding-model` | text-embedding-3-small | Modelo OpenAI (dimensão 1536 ou 3072 para large)                |
| `--url`             | localhost:8080         | URL base do servidor                                            |

**Métricas exibidas:** throughput (vetores/seg), tempo total. Relatório Markdown ao final.

### 2. Search (Read Stress)

Realiza buscas vetoriais de forma contínua por um tempo determinado. O modo usa uma **arquitetura de duas fases** para que a latência reportada seja **exclusivamente do FerresDB** (sem overhead de chamadas à API de embeddings dentro do loop de medição):

- **Fase 1 — Preparação:** Pré-computa um pool de vetores de query. Com `--openai-api-key`, gera N textos ("Query about topic 0..N...") e chama a API de embeddings da OpenAI em batch; sem OpenAI, gera N vetores aleatórios. Nenhuma medição é feita nesta fase.
- **Fase 2 — Benchmark:** Workers consomem vetores do pool (round-robin) e disparam apenas `POST .../search` contra o FerresDB. O timer mede só o tempo de cada requisição HTTP (ida e volta ao servidor).

Assim, QPS e latência (min, avg, P50/P90/P95/P99, max) refletem apenas a capacidade do FerresDB, e não da API OpenAI. Para benchmarks representativos do FerresDB puro, pode-se usar vetores aleatórios (`--dim 1536`). O modo OpenAI garante vetores realistas, mas a geração de embeddings é pré-computada na Fase 1 e **não** afeta a medição.

```bash
# Com API key FerresDB + OpenAI (pool de embeddings pré-computado)
ferres-bench search --api-key <ferres-api-key> --openai-api-key <openai-key> --duration 60s --concurrency 100

# Vetores aleatórios (sem OpenAI)
ferres-bench search --duration 60s --concurrency 100 --dim 768
```

| Argumento           | Default                | Descrição                                                       |
| ------------------- | ---------------------- | --------------------------------------------------------------- |
| `--api-key`         | —                      | API key do FerresDB (env: `FERRESDB_API_KEY`)                   |
| `--duration`        | 60s                    | Duração do teste (ex: `30s`, `2m`)                              |
| `--concurrency`     | 100                    | Número de workers concorrentes                                  |
| `--collection`      | bench                  | Nome da coleção (deve existir e ter dados)                      |
| `--dim`             | —                      | Dimensão do vetor de busca (com OpenAI vem do modelo)           |
| `--openai-api-key`  | —                      | Chave OpenAI para embedding das queries (env: `OPENAI_API_KEY`) |
| `--embedding-model` | text-embedding-3-small | Modelo OpenAI                                                   |
| `--num-queries`     | 200                    | Tamanho do pool de vetores de query (pré-computados na Fase 1)  |
| `--limit`           | 10                     | Número de resultados por busca                                  |
| `--warmup`          | 10                     | Requisições de aquecimento (não contabilizadas) antes da Fase 2 |
| `--url`             | localhost:8080         | URL base do servidor                                            |

**Métricas exibidas:** Query Pool, Total Requests, Successful (count e %), Duration, QPS, Latency Min/Avg/P50/P90/P95/P99/Max (HDR Histogram). Relatório Markdown ao final.

### 3. Chaos (Mixed)

Executa escritas, leituras e criações de coleção **simultaneamente** para testar robustez do RwLock e do WAL.

```bash
ferres-bench chaos --duration 30s --writers 20 --readers 50
```

| Argumento    | Default        | Descrição                    |
| ------------ | -------------- | ---------------------------- |
| `--duration` | 30s            | Duração do teste             |
| `--writers`  | 20             | Número de workers de escrita |
| `--readers`  | 50             | Número de workers de leitura |
| `--dim`      | 768            | Dimensão dos vetores         |
| `--url`      | localhost:8080 | URL base do servidor         |

**Métricas exibidas:** total de pontos escritos, total de buscas, coleções criadas. Relatório Markdown ao final.

## Exemplo de relatório (Markdown)

Ao final da execução, a ferramenta imprime um bloco em Markdown, por exemplo:

```markdown
---
## FerresDB Benchmark Report — Search

| Metric | Value |
|--------|-------|
| Mode | Search (Read Stress) |
| Query Pool | 200 vectors (OpenAI text-embedding-3-small) |
| Total Requests | 95,230 |
| Successful | 95,230 (100.0%) |
| Duration | 15.02s |
| QPS | 6,342 |
| Latency Min | 2.10ms |
| Latency Avg | 12.50ms |
| Latency P50 | 11.20ms |
| Latency P90 | 16.80ms |
| Latency P95 | 18.20ms |
| Latency P99 | 24.10ms |
| Latency Max | 45.00ms |

---
```

Pode ser copiado diretamente para issues, PRs ou posts (GitHub/LinkedIn).

## Objetivo técnico: Backpressure e Connection Pooling

A ferramenta foi desenhada para ajudar a validar se o sistema aguenta **muitas conexões simultâneas** (por exemplo, 10k) sem cair. Sugestões de uso:

1. **Aumentar concorrência progressivamente** em `ingest` e `search` (ex.: 50 → 200 → 1000).
2. **Medir P99** com `search`: o HDR Histogram fornece percentis precisos para avaliar tail latency.
3. **Modo chaos** para estressar RwLock e WAL com mistura de escritas, leituras e criação de coleções.

Garanta que o servidor está compilado em **release** e, se necessário, ajuste limites de recursos do SO (file descriptors, conexões) para testes com milhares de conexões.

## Variáveis de ambiente

- **FERRESDB_URL** — Sobrescreve o default de `--url` (ex.: `export FERRESDB_URL=http://prod:8080`).
- **FERRESDB_API_KEY** — API key do FerresDB quando `--api-key` não é passado (conforme [docs/api.md](api.md)).
- **OPENAI_API_KEY** — Chave da API OpenAI para `ingest` e `search` quando `--openai-api-key` não é passado.

## Dependências da crate

A crate `ferres-bench` utiliza: `tokio`, `clap`, `rand`, `indicatif`, `hdrhistogram`, `humantime` e `ferres-db-sdk` (REST). Nenhuma configuração extra é necessária além do workspace.
