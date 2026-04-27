---
name: Architectural Decisions
description: All recorded architectural decisions with context, rationale, and consequences
type: project
---

# FerresDB — Architectural Decisions

_All decisions below were originally documented in `docs/decisions.md`. This vault copy is the canonical reference. When adding new decisions, use the template in `valt/templates/decision.md` and also update `docs/decisions.md`. Last ADR: 022._

---

## [2024] ADR-001: Use `hnsw_rs` instead of custom HNSW implementation
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `Cargo.toml` workspace deps

**Context:** Needed an efficient ANN algorithm for vector search. Options: custom implementation, `usearch`, `faiss-rs`, or `hnsw_rs`.

**Decision:** Use the `hnsw_rs` library (native Rust, no FFI, actively maintained).

**Why not the alternatives:**
- Custom: too long to build, high bug risk
- `usearch`: fewer features, smaller community
- `faiss-rs`: C++ FFI, unnecessary complexity

**Consequences:** Faster time-to-value; external dependency but well-maintained. If specific features aren't supported, forking remains an option.

---

## [2024] ADR-002: JSON-lines format for persistence
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/storage.rs`

**Context:** Needed a persistence format supporting incremental append and easy debugging.

**Decision:** JSON-lines (`.jsonl`) for point storage. Binary snapshot (`points.bin` via bincode) added later as an opt-in via `StorageOptions::binary_snapshot`.

**Why not the alternatives:**
- Full bincode: no incremental append; hard to debug; partial recovery difficult
- SQLite: overkill overhead, unnecessary for simple data
- Protobuf: less readable; append complexity

**Consequences:** Human-readable; crash recovery via partial-file read; binary snapshot opt-in for performance.

---

## [2024] ADR-003: Trait Object for ANN index abstraction
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/search.rs`

**Context:** Needed to decouple `Collection` from a specific search backend.

**Decision:** `Box<dyn ANNIndex>` trait object. `ANNIndex` is object-safe.

**Why not the alternatives:**
- Generic `Collection<I: ANNIndex>`: hard to store in `HashMap`; type complexity
- Enum: less extensible; match statements everywhere

**Consequences:** Vtable overhead negligible (<1% of HNSW cost). Enables `HnswIndex`, `QuantizedHnswIndex`, `PolarQuantHnswIndex` without `Collection` changes.

---

## [2024] ADR-004: Tombstones for point removal
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/reindex.rs`

**Context:** HNSW does not support native node removal.

**Decision:** Mark removed points as tombstones; filter at search time. Automatic reindex when tombstone ratio > 20%.

**Why not the alternatives:**
- Full rebuild after each deletion: O(n log n), blocks operations
- Custom HNSW removal: very high complexity, high bug risk

**Consequences:** O(1) deletion; background reindex every 30 min or on demand via `POST /reindex`.

---

## [2024] ADR-005: L2 normalization for Cosine metric
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/lib.rs` (Cosine path in `upsert_points`)

**Decision:** Normalize vectors at insertion when metric = Cosine. This transforms Cosine search into L2, which is more efficient.

**Why not:**
- Native HNSW Cosine: less numerically stable, zero-norm issues
- Normalize only at search time: repeated overhead per query

**Consequences:** Stored vectors are normalized (original not preserved). Calculation in f64 to avoid overflow at high dimensions.

---

## [2024] ADR-006: Optional LRU cache per collection
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `CollectionConfig::search_cache_size`

**Decision:** Optional LRU cache, configurable per collection via `search_cache_size` (0 = disabled).

**Why not always-on:** Overhead even when queries never repeat; inflexible.

**Consequences:** 10–100× latency improvement for repeated queries. Stale results possible when points mutate (acceptable tradeoff).

---

## [2024] ADR-007: Conditional parallelization with Rayon for batches > 100 points
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/lib.rs:700-737`

**Decision:** Use `rayon` parallel iterators for validation and Cosine normalization when batch size > 100.

**Why not always parallel:** Overhead for small batches. HNSW insertion itself remains sequential (not thread-safe).

**Consequences:** ~50% improvement for 10k+ point batches with no overhead for small batches.

---

## [2024] ADR-008: Strict vector validation
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `FerresError` variants

**Decision:** Validate all vectors (empty, NaN/inf, dimension) at `Point::new` and `Collection` operations. Fail fast.

**Consequences:** Clear errors; prevents undefined HNSW behavior; validation overhead < 1%.

---

## [2024] ADR-009: Atomic writes with temp-file + rename
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `crates/core/src/storage.rs`

**Decision:** Write to temp file, then `rename()` atomically. All snapshot writes use this pattern.

**Consequences:** Atomicity guaranteed at filesystem level. No corruption on crash. NFS edge cases possible but rare.

---

## [2024] ADR-010: MD5 checksum for integrity validation
**Status:** ✅ Decided
**Inferred from:** `docs/decisions.md`, `Cargo.toml` (`md5 = "0.7"`)

**Decision:** Store MD5 checksum of `points.jsonl` for load-time corruption detection.

**Why MD5 over SHA-256:** Fast; adequate for integrity (not security). SHA-256 would be overkill here.

---

## [2026-02] ADR-011: WAL for durability (previously "proposed")
**Status:** ✅ Decided — implemented
**Inferred from:** `CHANGELOG.md`, `crates/core/src/wal.rs`

**Context:** Full snapshot after each operation was too slow for write-heavy workloads.

**Decision:** Append-only WAL (`wal.log`) with snapshot threshold (1000 ops). Each entry has a Unix timestamp. On startup, load last snapshot and replay WAL.

**Consequences:** Better write throughput; crash recovery; PITR support via timestamped entries.

---

## [2026-02] ADR-012: Scalar Quantization SQ8 for memory reduction
**Status:** ✅ Decided — implemented
**Inferred from:** `CHANGELOG.md`, `crates/core/src/quantization.rs`

**Decision:** Optional SQ8 compression (f32 → u8 per dimension, calibrated per-collection). `QuantizedHnswIndex` uses asymmetric distance (query f32 vs candidate u8) for precision.

**Consequences:** ~4× memory reduction. Recall@10 > 90% vs f32. QJL residual correction available for further recall improvement.

---

## [2026-02] ADR-013: Axum as the HTTP framework
**Status:** ✅ Decided
**Inferred from:** `crates/server/Cargo.toml` (`axum = "0.8"`), server structure

**Context:** Needed an async HTTP framework compatible with Tokio.

**Decision:** Axum 0.8 with Tower middleware. Routes split into `routes/` (registration) + `handlers/` (logic).

**Why not Actix-web:** Axum's Tower compatibility provides better middleware composition (CORS, trace, rate-limiting, body-limit via `DefaultBodyLimit`).

---

## [2026-02] ADR-014: SQLite for user/API-key persistence
**Status:** ✅ Decided
**Inferred from:** `crates/server/src/users.rs`, `crates/server/src/api_keys.rs`, `rusqlite` dep

**Context:** Needed persistent storage for users, API keys, and cloud settings without embedding a full DB.

**Decision:** SQLite via `rusqlite` (bundled). Three separate `.db` files: `users.db`, `api_keys.db`, `cloud_settings.db`.

**Consequences:** No external dependency; durable; simple ALTER TABLE migrations done in code.

---

## [2026-02] ADR-015: gRPC as optional feature (not default)
**Status:** ✅ Decided
**Inferred from:** `crates/server/Cargo.toml` (`grpc` feature), `crates/server/src/grpc.rs`

**Context:** gRPC adds significant build complexity (requires `protoc`). Most users need REST only.

**Decision:** gRPC compiled only with `--features grpc`. REST always available. gRPC reuses `AppState` and all core types — zero business logic duplication.

---

## [2026-02] ADR-016: Tiered storage Hot/Warm/Cold
**Status:** ✅ Decided
**Inferred from:** `CHANGELOG.md`, `crates/core/src/tiered.rs`

**Context:** Large collections require more memory than available; infrequently accessed points should move to cheaper storage.

**Decision:** Three tiers: Hot (RAM HashMap), Warm (mmap via `memmap2`), Cold (on-demand JSON disk). HNSW graph always stays in RAM. Access tracker triggers lazy promotion.

**Consequences:** Reduced RAM usage for large collections at cost of variable search latency depending on tier hit.

---

## [2026-02] ADR-017: Tokio runtime with custom stack size and blocking thread cap
**Status:** ✅ Decided
**Inferred from:** `crates/server/src/main.rs:35-41`

**Context:** HNSW search in `spawn_blocking` caused `STATUS_STACK_BUFFER_OVERRUN` at high concurrency.

**Decision:** 8 MiB thread stack (vs default 2 MiB) and max 128 blocking threads.

**Why:** Deep HNSW traversal + tracing frames in release builds exceeded default stack. 128 blocking threads provides back-pressure instead of unbounded thread creation.

---

## [2026-04-27] ADR-020: Server-side LLM proxy (no provider keys in the browser)
**Status:** ✅ Decided
**Decided by:** chore/002 / fix/005 (this session)

**Context:** O dashboard tinha `callOpenAI`/`callAnthropic`/`callGemini` em
`QueryTester.tsx` que faziam `fetch` direto do navegador para
`api.openai.com`, `api.anthropic.com` e `generativelanguage.googleapis.com`,
passando uma API key digitada pelo usuário no input "API Key". Isso expõe a
chave em DevTools/extensões/HAR captures e quebra a recomendação oficial dos
três provedores (todos pedem que a chave nunca toque o cliente).

**Decision:** Criar um proxy server-side (`POST /api/v1/llm/complete`) que
recebe `{ provider, model, prompt, max_tokens?, temperature? }` e fala com
OpenAI/Anthropic/Gemini do servidor. As credenciais vêm de env var
(`FERRESDB_{OPENAI,ANTHROPIC,GEMINI}_API_KEY`) ou da tabela SQLite
`llm_credentials(provider PK, api_key, updated_at)` gerenciada por Admin.
Env tem precedência. RBAC: Admin OU Editor podem usar o proxy. Endpoints
admin (`GET/PUT/DELETE /api/v1/admin/llm-credentials/{provider}`) nunca
retornam o valor da chave — só `configured`/`source`. Audit grava
`action="llm_complete"` com `model`, `prompt_bytes` e `key_source`, **sem o
conteúdo do prompt**. Métrica Prometheus
`ferresdb_llm_proxy_requests_total{provider, status}` (status: ok,
upstream_error, auth_error, timeout, error).

**Why not the alternatives:**
- **Manter chamadas client-side:** vetor de exposição direto da chave; nenhuma
  forma de auditar uso por usuário; quebra recomendação dos provedores.
- **Apenas env var (sem DB):** Admin precisaria de acesso SSH para reconfigurar.
  DB permite gerenciar pelo dashboard com audit completo, mantendo env como
  override "always wins".
- **Mostrar a chave em GET após PUT:** GET nunca retorna a chave — apenas
  `configured`/`source`. Isso evita acidentalmente vazar a chave em logs do
  navegador, history do react-query, etc.
- **Dispatcher genérico (sem enum):** o enum `LlmProvider` em Rust dá rejeição
  de payload inválido pelo serde no parsing (4xx automático) e força o
  exhaustiveness no `match` do handler, eliminando uma classe inteira de bugs.

**Consequences:**
- Frontend do `QueryTester.tsx` chama `llmApi.complete(...)` em vez de fetch
  direto; o input "API Key" foi rotulado como "Embedding API Key" e sua
  descrição deixa claro que keys de LLM são server-side.
- Embedding (`useEmbedding.ts`) **não** foi proxado neste PR — escopo
  expressamente limitado pela instrução. Chave de embedding ainda é
  client-side; pode ser proxada num PR seguinte.
- Wiremock + `serial_test` para testes de integração (env var
  `FERRESDB_LLM_PROXY_BASE_URL` redireciona requests para mock).
- Nova dependência principal `reqwest` no crate `server` (antes era só
  dev-dep). Aumenta marginalmente o tempo de build, mas elimina necessidade de
  rolar um cliente HTTP manual.

---

## [2026-04-27] ADR-019: Centralized clock-safe Unix time helpers
**Status:** ✅ Decided
**Decided by:** chore/002 (this session)

**Context:** O padrão `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()`
estava espalhado em ~45 call sites entre `core` e `server`. Em clock skew
(VMs/containers onde o relógio retrocede antes de `UNIX_EPOCH`), esse `unwrap`
gera panic e derruba o servidor inteiro. Vetor de falha real, ainda que raro.

**Decision:** Centralizar acesso ao relógio em `crates/core/src/time.rs` com
helpers `unix_now() -> u64`, `unix_now_millis() -> u128` e
`unix_duration() -> Duration`. Em caso de relógio retrocedido, retornam `0` /
`Duration::ZERO` em vez de panic. `crates/server/src/time.rs` re-exporta o
helper do core (server depende de core; core não pode depender de server).

**Why not the alternatives:**
- **Manter o `unwrap` e confiar no relógio:** falha real observada em containers;
  o custo de degradar para `0` é baixíssimo.
- **Retornar `Result<u64, _>` em todo call site:** invasivo demais; a maioria dos
  call sites usa o timestamp como tag/log onde `0` é aceitável.
- **Helper só em `core` sem re-export no server:** funciona, mas força cada
  importador no server a saber o caminho do core; o re-export mantém o ergonomic
  `crate::time::unix_now`.

**Consequences:**
- Servidor não sofre mais panic por clock skew em nenhum dos call sites
  centralizados.
- Benchmarks (`crates/core/benches/`, `crates/benchmark`) **não** usam o helper
  por desenho — neles o panic barulhento é desejado.
- `Mutex::lock().unwrap()` e outros `unwrap`s legítimos ficam para PRs futuros.

---

## [2026-04] ADR-018: Brute-force fallback for small collections
**Status:** ✅ Decided
**Inferred from:** Recent commits: `d45ad9f fix: brute-force fallback for small collections in Collection::search`, `0f7e7d4 fix: gate brute-force search on full in-RAM coverage`

**Context:** HNSW is unreliable on very small collections (few points, sparse graph). Tests were flaky.

**Decision:** Use brute-force search when collection is small enough to fit entirely in RAM AND HNSW graph is not sufficiently dense.

**Consequences:** Stable test results for small fixtures; HNSW still used for all production-scale collections.

---

## [2026-04-27] ADR-021: HTTP security headers via dynamic nginx snippet
**Status:** ✅ Decided
**Decided by:** Rafael Ferres

**Context:** The production Nginx server had only three security headers (X-Frame-Options SAMEORIGIN, X-Content-Type-Options, X-XSS-Protection). CSP, Referrer-Policy, and Permissions-Policy were absent, leaving the dashboard exposed to clickjacking, XSS, and information leakage. The CSP `connect-src` also needed to be configurable at runtime (FERRESDB_API_URL) without an image rebuild.

**Decision:** Move all security headers out of `nginx.conf` into a generated snippet `/etc/nginx/snippets/security_headers.conf` created by `docker-entrypoint.sh`. The snippet is included at server level and repeated in every `location` block that defines its own `add_header` (nginx drops parent `add_header` directives for any location that has its own). The CSP `connect-src` is built dynamically: when `FERRESDB_API_URL` is set, that origin is prepended before `ws: wss:`.

**Why not the alternatives:**
- **envsubst on nginx.conf template:** `nginx.conf` uses `$` for nginx variables (`$uri`, etc.); even the selective `envsubst '${VAR}'` form requires careful quoting and is error-prone.
- **sed -i on nginx.conf:** Mutates the file in-place; not idempotent across container restarts without recreation.
- **nginx `set $csp_default` variable:** Solves the header value but not the location-inheritance problem; still requires the snippet approach for correct per-location delivery.

**Consequences:** Every response (HTML, JS, CSS, config.js) gets the full set of security headers. Adding future API origins requires only setting `FERRESDB_API_URL` in the compose file. The `X-XSS-Protection` header was intentionally dropped (deprecated since Chrome 78, can cause issues in some browsers). The snippet file is regenerated on every container start, so it is always consistent with current env vars.

---

## [2026-04-27] ADR-022: WAL durability policy — fsync per write by default
**Status:** ✅ Decided
**Decided by:** Rafael Ferres
**Inferred from:** `crates/core/src/wal.rs`, spec at `docs/superpowers/specs/2026-04-27-wal-durability-design.md`

**Context:** The WAL only called `BufWriter::flush()` — data landed in OS page cache but was never synced to disk. A kernel panic or power loss could lose recent writes. The tiered storage module already used `sync_all()`, but the WAL (the primary crash recovery mechanism) had no durability guarantee.

**Decision:** Added `WalConfig` with `fsync_per_write` flag (default: `true` in production). Uses `sync_data()` (not `sync_all()`) for speed. When `fsync_per_write=false`, periodic fsync by op count or time interval. Inline check in `append_entry()` — no background thread.

**Why not the alternatives:**
- **Background fsync thread:** Rejected — adds `Arc<Mutex<File>>` contention on every write, more complex. Inline check has slight latency jitter but no lock overhead.
- **`sync_all()` instead of `sync_data()`:** Rejected — `sync_data()` is faster (skips metadata flush) and sufficient for data durability on all common filesystems.

**Consequences:** Every `append_*` call is ~2-3x slower on SSD, ~10x on HDD. Production data is now durable against kernel panic. Can be disabled for benchmarks via `FERRESDB_WAL_FSYNC_PER_WRITE=false`.

---

## Relacionado
- [[overview]] — stack e arquitetura geral
- [[conventions]] — como as decisões se traduzem em código
- [[hnsw-is-approximate-by-design]] — learning: ADR-018 em detalhe
- [[tiered-storage-breaks-points-len-assumption]] — learning: ADR-016 em detalhe
- [[tokio-stack-size-for-hnsw]] — learning: ADR-017 em detalhe
