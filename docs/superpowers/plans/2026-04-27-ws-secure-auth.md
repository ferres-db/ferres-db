# WebSocket Secure Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop leaking JWT/API keys in WebSocket query strings by adding subprotocol-based auth as the preferred method, keeping `Authorization` header as a secondary path, and deprecating (with visible warnings and response headers) the insecure `?token=` query param.

**Architecture:** `authenticate_ws` is refactored to return `Option<WsAuthMethod>` (an enum with three variants). `ws_handler` reads that variant to emit a Prometheus metric, and — only for `QueryParam` — appends `Deprecation: true` + `Sunset: <HTTP-date>` to the 101 response. The frontend switches from `new WebSocket(url?token=…)` to `new WebSocket(url, ["ferresdb.auth.bearer.<token>", "ferresdb.v1"])` via a new helper. `getWsUrl` is removed entirely (only one caller existed: `useWebSocket.ts`).

**Tech Stack:** Rust/Axum 0.8 (server), chrono (HTTP date), tokio-tungstenite 0.24 (tests), TypeScript/React (frontend).

---

## File Map

| Action   | Path                                              | What changes                                   |
|----------|---------------------------------------------------|------------------------------------------------|
| Modify   | `crates/server/src/metrics.rs`                    | Add `WS_AUTH_METHOD_TOTAL` CounterVec          |
| Modify   | `crates/server/src/handlers/streaming.rs`         | Refactor `authenticate_ws`; update `ws_handler`|
| Modify   | `crates/server/tests/websocket_test.rs`           | New subprotocol tests; migrate existing tests  |
| Modify   | `dashboard/src/api/ferresdb.ts`                   | Add `createAuthenticatedWs`; remove `getWsUrl` |
| Modify   | `dashboard/src/hooks/useWebSocket.ts`             | Use `createAuthenticatedWs` instead of `getWsUrl` |
| Modify   | `docs/api.md`                                     | Add WebSocket Authentication section           |

---

## Task 1 — Add `WS_AUTH_METHOD_TOTAL` metric

**Files:**
- Modify: `crates/server/src/metrics.rs`

- [ ] **Step 1: Add the counter inside the `lazy_static!` block**

In `crates/server/src/metrics.rs`, after the `WS_MESSAGES_SENT_TOTAL` entry (line 68), add:

```rust
    /// Contador de autenticações WebSocket por método.
    /// `method` valores: "subprotocol", "header", "query".
    pub static ref WS_AUTH_METHOD_TOTAL: CounterVec = register_counter_vec!(
        "ferresdb_ws_auth_method_total",
        "WebSocket authentication method used",
        &["method"]
    ).unwrap();
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check -p ferres-db-server
```
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/metrics.rs
git commit -m "feat(metrics): add ferresdb_ws_auth_method_total counter"
```

---

## Task 2 — Write failing test: subprotocol auth

**Files:**
- Modify: `crates/server/tests/websocket_test.rs`

- [ ] **Step 1: Add the test at the bottom of the file**

```rust
/// Testa conexão WebSocket autenticada via Sec-WebSocket-Protocol.
#[tokio::test]
async fn test_ws_auth_via_subprotocol() {
    let server = setup_server().await;
    create_collection(&server.base_url, "ws-test-subproto", 3).await;

    use tokio_tungstenite::tungstenite::http::Request;

    let request = Request::builder()
        .uri(format!("{}/api/v1/ws", server.ws_url))
        .header(
            "Sec-WebSocket-Protocol",
            format!("ferresdb.auth.bearer.{TEST_API_KEY}, ferresdb.v1"),
        )
        .body(())
        .unwrap();

    let (ws_stream, response) = connect_async(request)
        .await
        .expect("failed to connect via subprotocol");

    // Server must echo back ferresdb.v1 (without the token)
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok()),
        Some("ferresdb.v1"),
        "server must respond with ferresdb.v1 subprotocol"
    );

    let (mut write, mut read) = ws_stream.split();

    let msg = serde_json::json!({
        "type": "upsert",
        "collection": "ws-test-subproto",
        "points": [{"id": "sp-1", "vector": [0.1, 0.2, 0.3], "metadata": {}}]
    });
    write
        .send(Message::Text(msg.to_string()))
        .await
        .expect("failed to send upsert");

    let ack = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Ok(m)) = read.next().await {
                let text = m.into_text().unwrap_or_default();
                let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                if json["type"] == "ack" {
                    return json;
                }
            }
        }
    })
    .await
    .expect("timeout waiting for ack");

    assert_eq!(ack["upserted"], 1);
    write.close().await.ok();
}

/// Testa que ?token= retorna Deprecation: true e Sunset no header da resposta.
#[tokio::test]
async fn test_ws_query_param_deprecated_headers() {
    let server = setup_server().await;

    let url = format!("{}/api/v1/ws?token={}", server.ws_url, TEST_API_KEY);
    let (ws_stream, response) = connect_async(&url)
        .await
        .expect("failed to connect via query param");

    assert_eq!(
        response
            .headers()
            .get("deprecation")
            .and_then(|v| v.to_str().ok()),
        Some("true"),
        "expected Deprecation: true header"
    );
    assert!(
        response.headers().get("sunset").is_some(),
        "expected Sunset header"
    );

    let (mut write, _) = ws_stream.split();
    write.close().await.ok();
}
```

- [ ] **Step 2: Run — must FAIL**

```bash
cargo test -p ferres-db-server test_ws_auth_via_subprotocol -- --nocapture 2>&1 | tail -20
```
Expected: FAIL (server still returns 401 because subprotocol auth isn't implemented yet, OR connection is refused). Either error is correct at this stage.

```bash
cargo test -p ferres-db-server test_ws_query_param_deprecated_headers -- --nocapture 2>&1 | tail -20
```
Expected: FAIL — `Deprecation` header missing.

---

## Task 3 — Implement `authenticate_ws` refactor + `ws_handler` update

**Files:**
- Modify: `crates/server/src/handlers/streaming.rs`

- [ ] **Step 1: Replace the top-of-file doc comment**

Change the first line from:
```rust
//! Suporta batch automático (debounce 10ms) e autenticação via
//! query param `?token=sk-xxx` ou header `Authorization: Bearer <key>`.
```
to:
```rust
//! Suporta batch automático (debounce 10ms). Autenticação aceita três métodos:
//! 1. Sec-WebSocket-Protocol: ferresdb.auth.bearer.<token>, ferresdb.v1 (preferido)
//! 2. Authorization: Bearer <token> (clientes nativos)
//! 3. ?token=<token> (DEPRECATED — será removido na v0.2)
```

- [ ] **Step 2: Update the imports block**

Replace the current `use axum::{...}` block with:

```rust
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
};
use chrono::{Duration as ChronoDuration, Utc};
```

- [ ] **Step 3: Add `WsAuthMethod` enum — insert after the `WsQueryParams` struct**

After line:
```rust
pub struct WsQueryParams {
    pub token: Option<String>,
}
```

Add:
```rust
/// Método pelo qual o cliente se autenticou no WebSocket.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WsAuthMethod {
    /// Sec-WebSocket-Protocol: ferresdb.auth.bearer.<token>, ferresdb.v1
    Subprotocol,
    /// Authorization: Bearer <token>
    Header,
    /// ?token=<token> — deprecated
    QueryParam,
}
```

- [ ] **Step 4: Replace the `authenticate_ws` function and its helpers**

Delete the existing `authenticate_ws` (lines 116–167) and `is_valid_legacy_key` (lines 169–185) functions. Replace with:

```rust
/// Valida um token contra todos os mecanismos conhecidos (API keys, legacy, JWT).
fn validate_token(token: &str) -> bool {
    crate::api_keys::ApiKeyStore::validate(token)
        || is_valid_legacy_key(token)
        || crate::auth::validate_jwt(token)
}

/// Retorna o método de autenticação usado, ou `None` se inválido.
fn authenticate_ws(headers: &HeaderMap, query: &WsQueryParams) -> Option<WsAuthMethod> {
    // 1. Sec-WebSocket-Protocol: ferresdb.auth.bearer.<token>, ferresdb.v1
    if let Some(proto) = headers
        .get("Sec-WebSocket-Protocol")
        .and_then(|h| h.to_str().ok())
    {
        for part in proto.split(',') {
            if let Some(token) = part.trim().strip_prefix("ferresdb.auth.bearer.") {
                if validate_token(token) {
                    return Some(WsAuthMethod::Subprotocol);
                }
            }
        }
    }

    // 2. Authorization: Bearer <token>
    if let Some(auth) = headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
    {
        if let Some(key) = auth.strip_prefix("Bearer ") {
            if validate_token(key) {
                return Some(WsAuthMethod::Header);
            }
        }
    }

    // 3. Query param (deprecated)
    if let Some(token) = &query.token {
        if validate_token(token) {
            return Some(WsAuthMethod::QueryParam);
        }
    }

    None
}

/// Check legacy API key validity against FERRESDB_API_KEYS env var.
fn is_valid_legacy_key(key: &str) -> bool {
    use std::collections::HashSet as HS;
    use std::sync::OnceLock;
    static LEGACY_KEYS: OnceLock<HS<String>> = OnceLock::new();
    let keys = LEGACY_KEYS.get_or_init(|| {
        std::env::var("FERRESDB_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    keys.contains(key)
}

/// Formata uma data HTTP (RFC 7231) para daqui 180 dias.
fn sunset_http_date() -> String {
    (Utc::now() + ChronoDuration::days(180))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}
```

- [ ] **Step 5: Replace the `ws_handler` auth block and upgrade call**

Find the authentication and upgrade block in `ws_handler` (roughly lines 209–219):
```rust
    // Autenticação
    if !authenticate_ws(&headers, &query) {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid or missing API key",
        )
            .into_response();
    }

    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_ws_connection(socket, app_state))
```

Replace with:
```rust
    // Autenticação
    let Some(auth_method) = authenticate_ws(&headers, &query) else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid or missing API key",
        )
            .into_response();
    };

    let method_label = match auth_method {
        WsAuthMethod::Subprotocol => "subprotocol",
        WsAuthMethod::Header => "header",
        WsAuthMethod::QueryParam => "query",
    };
    crate::metrics::WS_AUTH_METHOD_TOTAL
        .with_label_values(&[method_label])
        .inc();

    let mut response = ws
        .protocols(["ferresdb.v1"])
        .max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_ws_connection(socket, app_state))
        .into_response();

    if auth_method == WsAuthMethod::QueryParam {
        warn!("WebSocket auth via query param is deprecated and will be removed in v0.2");
        response
            .headers_mut()
            .insert("deprecation", HeaderValue::from_static("true"));
        if let Ok(v) = HeaderValue::from_str(&sunset_http_date()) {
            response.headers_mut().insert("sunset", v);
        }
    }

    response
```

- [ ] **Step 6: Verify it compiles**

```bash
cargo check -p ferres-db-server
```
Expected: no errors.

- [ ] **Step 7: Run the two new tests — must PASS now**

```bash
cargo test -p ferres-db-server test_ws_auth_via_subprotocol -- --nocapture 2>&1 | tail -20
```
Expected: PASS.

```bash
cargo test -p ferres-db-server test_ws_query_param_deprecated_headers -- --nocapture 2>&1 | tail -20
```
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/server/src/handlers/streaming.rs
git commit -m "feat(ws): multi-method auth — subprotocol > header > query param (deprecated)"
```

---

## Task 4 — Migrate existing WS tests to use subprotocol

**Files:**
- Modify: `crates/server/tests/websocket_test.rs`

The four existing tests (`test_ws_upsert_10_points`, `test_ws_ping_pong`, `test_ws_upsert_collection_not_found`, `test_ws_subscribe_and_receive_event`, `test_ws_invalid_message`) all connect with:
```rust
let url = format!("{}/api/v1/ws?token={}", server.ws_url, TEST_API_KEY);
let (ws_stream, _response) = connect_async(&url).await.expect("...");
```

- [ ] **Step 1: Add a test helper to `websocket_test.rs` above the tests block**

After the `rest_upsert_points` helper (line ~125), add:

```rust
/// Cria uma conexão WebSocket autenticada via Sec-WebSocket-Protocol.
async fn connect_authenticated(ws_url: &str) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
> {
    use tokio_tungstenite::tungstenite::http::Request;

    let request = Request::builder()
        .uri(format!("{ws_url}/api/v1/ws"))
        .header(
            "Sec-WebSocket-Protocol",
            format!("ferresdb.auth.bearer.{TEST_API_KEY}, ferresdb.v1"),
        )
        .body(())
        .unwrap();

    let (ws_stream, _) = connect_async(request)
        .await
        .expect("connect_authenticated: failed to connect");
    ws_stream
}
```

- [ ] **Step 2: Migrate `test_ws_upsert_10_points`**

Replace:
```rust
    let url = format!("{}/api/v1/ws?token={}", server.ws_url, TEST_API_KEY);
    let (ws_stream, _response) = connect_async(&url).await.expect("failed to connect");
```
With:
```rust
    let ws_stream = connect_authenticated(&server.ws_url).await;
```

- [ ] **Step 3: Migrate `test_ws_ping_pong`**

Replace:
```rust
    let url = format!("{}/api/v1/ws?token={}", server.ws_url, TEST_API_KEY);
    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
```
With:
```rust
    let ws_stream = connect_authenticated(&server.ws_url).await;
```

- [ ] **Step 4: Migrate `test_ws_upsert_collection_not_found`**

Same replacement as Step 2.

- [ ] **Step 5: Migrate `test_ws_subscribe_and_receive_event`**

Replace:
```rust
    let url = format!("{}/api/v1/ws?token={}", server.ws_url, TEST_API_KEY);
    let (ws_stream, _) = connect_async(&url).await.expect("failed to connect");
```
With:
```rust
    let ws_stream = connect_authenticated(&server.ws_url).await;
```

- [ ] **Step 6: Migrate `test_ws_invalid_message`**

Same replacement as Step 2.

- [ ] **Step 7: Run all WS tests**

```bash
cargo test -p ferres-db-server --test websocket_test -- --nocapture 2>&1 | tail -40
```
Expected: all 7 tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/server/tests/websocket_test.rs
git commit -m "test(ws): migrate tests from ?token= to Sec-WebSocket-Protocol"
```

---

## Task 5 — Frontend: `createAuthenticatedWs` + remove `getWsUrl`

**Files:**
- Modify: `dashboard/src/api/ferresdb.ts`

- [ ] **Step 1: Replace `getWsUrl` with `createAuthenticatedWs`**

Find the current `getWsUrl` function (last ~8 lines of the file):
```typescript
// WebSocket URL helper
export function getWsUrl(token?: string): string {
  const base = API_BASE_URL || window.location.origin;
  const wsProtocol = base.startsWith("https") ? "wss" : "ws";
  const httpStripped = base.replace(/^https?:\/\//, "");
  const authToken = token || getStoredToken() || getApiKey() || "";
  return `${wsProtocol}://${httpStripped}/api/v1/ws?token=${encodeURIComponent(authToken)}`;
}
```

Replace it entirely with:
```typescript
/**
 * Creates an authenticated WebSocket using Sec-WebSocket-Protocol.
 * Token is read from session storage (login JWT) or runtime API key.
 * Never puts credentials in the URL.
 */
export function createAuthenticatedWs(path: string): WebSocket {
  const token = getStoredToken() || getApiKey() || "";
  const base = (API_BASE_URL || window.location.origin).replace(/^http/, "ws");
  const url = `${base}${path}`;
  return new WebSocket(url, [`ferresdb.auth.bearer.${token}`, "ferresdb.v1"]);
}
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd dashboard && npm run build 2>&1 | tail -20
```
Expected: no TypeScript errors (there will be a build error in `useWebSocket.ts` because `getWsUrl` is no longer exported — that's expected and is fixed in Task 6).

- [ ] **Step 3: Commit (do not yet — wait for Task 6)**

We'll commit both files together in Task 6.

---

## Task 6 — Update `useWebSocket.ts`

**Files:**
- Modify: `dashboard/src/hooks/useWebSocket.ts`

- [ ] **Step 1: Replace the import and the `connect` function body**

Change the import on line 2 from:
```typescript
import { getWsUrl } from "@/api/ferresdb";
```
to:
```typescript
import { createAuthenticatedWs } from "@/api/ferresdb";
```

- [ ] **Step 2: Update the `connect` callback inside `useWebSocket`**

Find (lines 72–81):
```typescript
  const connect = useCallback(
    (token?: string) => {
      // Clean up existing connection
      disconnect();

      setStatus("connecting");
      setError(null);

      const url = getWsUrl(token);
      const ws = new WebSocket(url);
```

Replace with:
```typescript
  const connect = useCallback(
    (_token?: string) => {
      // Clean up existing connection
      disconnect();

      setStatus("connecting");
      setError(null);

      const ws = createAuthenticatedWs("/api/v1/ws");
```

The `_token` parameter is kept (underscore-prefixed to signal unused) so the public API of `useWebSocket.connect(token?)` remains unchanged for any callers.

- [ ] **Step 3: Verify TypeScript compiles clean**

```bash
cd dashboard && npm run build 2>&1 | tail -20
```
Expected: no errors.

- [ ] **Step 4: Commit both frontend files**

```bash
git add dashboard/src/api/ferresdb.ts dashboard/src/hooks/useWebSocket.ts
git commit -m "feat(dashboard): use Sec-WebSocket-Protocol for WS auth, remove ?token= from URL"
```

---

## Task 7 — Document WebSocket Authentication in `docs/api.md`

**Files:**
- Modify: `docs/api.md`

- [ ] **Step 1: Find the WebSocket section in docs/api.md**

Search for existing WebSocket content:
```bash
grep -n -i "websocket\|ws_handler\|/api/v1/ws" docs/api.md | head -20
```

- [ ] **Step 2: Add "WebSocket Authentication" section**

Locate the `## Streaming (WebSocket)` section (or wherever `/api/v1/ws` is documented). Immediately after the endpoint description, insert the following block:

```markdown
### WebSocket Authentication

The server accepts three authentication methods for the WebSocket endpoint, evaluated in order of preference:

#### 1. `Sec-WebSocket-Protocol` (recommended)

The browser-safe method. The client offers two protocols: one carrying the token and the static `ferresdb.v1` identifier.

```
Sec-WebSocket-Protocol: ferresdb.auth.bearer.<token>, ferresdb.v1
```

The server validates the token and echoes back `ferresdb.v1` (without the token) in the 101 response:

```
Sec-WebSocket-Protocol: ferresdb.v1
```

**JavaScript (browser):**
```javascript
const ws = new WebSocket("wss://host/api/v1/ws", [
  "ferresdb.auth.bearer.<token>",
  "ferresdb.v1",
]);
```

#### 2. `Authorization` header

Some native clients (curl, Python `websockets`) can send HTTP headers during the WebSocket handshake:

```
Authorization: Bearer <token>
```

#### 3. Query parameter ⚠️ DEPRECATED

```
GET /api/v1/ws?token=<token>
```

> **Deprecated since v0.1. Scheduled for removal in v0.2.**
> Tokens in query strings leak into proxy logs, browser history, and `Referer` headers.
> When used, the server logs a warning and includes these headers in the 101 response:
> ```
> Deprecation: true
> Sunset: <HTTP-date 180 days in the future>
> ```

#### Observability

The metric `ferresdb_ws_auth_method_total{method}` counts connections by authentication method (`subprotocol`, `header`, `query`). Use it to track migration away from the deprecated query param path.
```

- [ ] **Step 3: Commit**

```bash
git add docs/api.md
git commit -m "docs(api): add WebSocket Authentication section with 3-method hierarchy"
```

---

## Task 8 — Run full test suite

- [ ] **Step 1: Run workspace tests targeting the server crate**

```bash
cargo test --workspace -p ferres-db-server 2>&1 | tail -40
```
Expected: all tests pass, no compiler errors.

- [ ] **Step 2: Run the full workspace to catch any downstream breakage**

```bash
cargo test --workspace 2>&1 | tail -40
```
Expected: all tests pass.

- [ ] **Step 3: Confirm metric is visible in Prometheus output**

```bash
cargo run --bin ferres-db-server &
sleep 2
# Connect once via subprotocol, once via header, once via query param, then:
curl -s http://localhost:8080/metrics | grep ws_auth_method
# Expected: three lines like
# ferresdb_ws_auth_method_total{method="subprotocol"} 1
# ferresdb_ws_auth_method_total{method="header"} 1
# ferresdb_ws_auth_method_total{method="query"} 1
kill %1
```

---

## ADR Note

After completing all tasks, append the following to `valt/decisions.md` and `docs/decisions.md`:

```markdown
## [2026-04-27] ADR-022: Sec-WebSocket-Protocol for credential-free WS auth
**Status:** ✅ Decided

**Context:** `?token=` query param leaks credentials into proxy logs, browser history,
and Referer headers. Browser native WebSocket API does not support custom HTTP headers
during the handshake, making `Authorization: Bearer` only available for native clients.

**Decision:** Primary auth method is `Sec-WebSocket-Protocol: ferresdb.auth.bearer.<token>, ferresdb.v1`.
Server echoes `ferresdb.v1`. Three methods in priority order:
1. Subprotocol (preferred, browser-safe)
2. Authorization header (native clients)
3. Query param (deprecated, removed in v0.2)

**Consequences:** `?token=` still works with `Deprecation: true` / `Sunset` response headers
and a server-side `warn!` log. Metric `ferresdb_ws_auth_method_total{method}` tracks adoption.
Frontend never puts token in URL. `getWsUrl` removed; callers use `createAuthenticatedWs(path)`.
```
