//! # Streaming Handlers — ingestão em tempo real via WebSocket
//!
//! Protocolo JSON sobre WebSocket para upsert de pontos, subscrição de eventos
//! e heartbeat. Suporta batch automático (debounce 10ms) e autenticação via
//! query param `?token=sk-xxx` ou header `Authorization: Bearer <key>`.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::HeaderMap,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::time::{self, interval};
use tracing::{debug, info, warn};

use crate::state::{AppState, CollectionEvent};
use crate::time::unix_now;

// ─── Constants ──────────────────────────────────────────────────────────

/// Máximo tamanho de mensagem WebSocket (10 MB).
const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Intervalo de heartbeat (ping) em segundos.
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Timeout para pong após ping (segundos).
/// Implicitly enforced: if pong doesn't arrive within one HEARTBEAT_INTERVAL, connection is closed.
#[allow(dead_code)]
const PONG_TIMEOUT_SECS: u64 = 10;

/// Timeout de inatividade (5 minutos).
const INACTIVITY_TIMEOUT_SECS: u64 = 5 * 60;

/// Intervalo de debounce para batch automático (ms).
const BATCH_DEBOUNCE_MS: u64 = 10;

// ─── Protocol Types ──────────────────────────────────────────────────────

/// Query params para autenticação WebSocket.
#[derive(Debug, Deserialize)]
pub struct WsQueryParams {
    pub token: Option<String>,
}

/// Mensagem do cliente para o servidor.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Upsert {
        collection: String,
        points: Vec<WsPointInput>,
    },
    Subscribe {
        collection: String,
        #[serde(default)]
        events: Vec<String>,
    },
    Ping,
}

/// Ponto enviado via WebSocket.
#[derive(Debug, Deserialize)]
struct WsPointInput {
    id: String,
    vector: Vec<f32>,
    #[serde(default)]
    metadata: serde_json::Value,
    /// TTL em segundos; se presente, o ponto expira após esse tempo.
    #[serde(default)]
    ttl: Option<u64>,
}

/// Mensagem do servidor para o cliente.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Ack {
        upserted: usize,
        failed: usize,
        took_ms: u64,
    },
    Event {
        collection: String,
        action: String,
        point_ids: Vec<String>,
        timestamp: u64,
    },
    Error {
        message: String,
        code: u16,
    },
    Pong,
}

impl ServerMessage {
    fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"type":"error","message":"serialization failed","code":500}"#.to_string()
        })
    }
}

// ─── Auth helper ─────────────────────────────────────────────────────────

/// Valida o token do WebSocket (query param ou header).
fn authenticate_ws(headers: &HeaderMap, query: &WsQueryParams) -> bool {
    // 1. Query param ?token=sk-xxx
    if let Some(ref token) = query.token {
        if crate::api_keys::ApiKeyStore::validate(token) {
            return true;
        }
        // Check legacy keys
        crate::auth::init_api_keys_from(None); // ensure initialized
                                               // Try legacy validation via the same mechanism the middleware uses
    }

    // 2. Header Authorization: Bearer <key>
    if let Some(auth_header) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
        if let Some(key) = auth_header.strip_prefix("Bearer ") {
            if crate::api_keys::ApiKeyStore::validate(key) {
                return true;
            }
        }
    }

    // 3. Query param against legacy keys
    if let Some(ref token) = query.token {
        // Check if it's in the legacy static set
        if is_valid_legacy_key(token) {
            return true;
        }
    }

    // 4. Header against legacy keys
    if let Some(auth_header) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
        if let Some(key) = auth_header.strip_prefix("Bearer ") {
            if is_valid_legacy_key(key) {
                return true;
            }
        }
    }

    // 5. JWT (login do dashboard) — query param ou header
    let token = query.token.as_deref().or_else(|| {
        headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
    });
    if let Some(t) = token {
        if crate::auth::validate_jwt(t) {
            return true;
        }
    }

    false
}

/// Check legacy API key validity (mirrors auth.rs logic).
fn is_valid_legacy_key(key: &str) -> bool {
    use std::collections::HashSet as HS;
    use std::sync::OnceLock;
    // Access the same OnceLock used in auth.rs
    // Since we can't access the private static, we use the env var directly
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

// ─── WebSocket Handler ──────────────────────────────────────────────────

/// Handler para GET /api/v1/ws — upgrade HTTP → WebSocket.
///
/// Autenticação: aceita API key via query param `?token=sk-xxx` ou header
/// `Authorization: Bearer <key>`.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(app_state): State<AppState>,
    Query(query): Query<WsQueryParams>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Verifica limite de conexões
    let current = app_state.ws_connections_active.load(Ordering::Relaxed);
    if current >= app_state.max_ws_connections {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "too many WebSocket connections",
        )
            .into_response();
    }

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
}

/// Processa uma conexão WebSocket individual.
async fn handle_ws_connection(socket: WebSocket, app_state: AppState) {
    // Incrementa gauge de conexões ativas
    app_state
        .ws_connections_active
        .fetch_add(1, Ordering::Relaxed);
    crate::metrics::WS_CONNECTIONS_ACTIVE.inc();

    info!("WebSocket connection established");

    let (mut sender, mut receiver) = socket.split();

    // Estado da conexão
    let mut subscriptions: HashSet<String> = HashSet::new();
    let mut subscription_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Canal para enviar mensagens de volta ao cliente (multiplex subscriptions + respostas)
    let (tx_out, mut rx_out) = tokio::sync::mpsc::channel::<String>(256);

    // Task de escrita: lê do mpsc e envia ao WebSocket
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx_out.recv().await {
            crate::metrics::WS_MESSAGES_SENT_TOTAL
                .with_label_values(&["outgoing"])
                .inc();
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Heartbeat e inactivity tracking
    let last_activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(unix_now()));
    let awaiting_pong = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Heartbeat task
    let tx_heartbeat = tx_out.clone();
    let last_activity_hb = last_activity.clone();
    let awaiting_pong_hb = awaiting_pong.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut hb_interval = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        loop {
            hb_interval.tick().await;

            // Check inactivity timeout
            let now = unix_now();
            let last = last_activity_hb.load(Ordering::Relaxed);
            if now.saturating_sub(last) > INACTIVITY_TIMEOUT_SECS {
                warn!("WebSocket inactivity timeout, closing");
                let _ = tx_heartbeat
                    .send(
                        ServerMessage::Error {
                            message: "inactivity timeout".to_string(),
                            code: 408,
                        }
                        .to_text(),
                    )
                    .await;
                break;
            }

            // Check pong timeout
            if awaiting_pong_hb.load(Ordering::Relaxed) {
                // We were waiting for pong — check if it arrived
                // (If still awaiting after a full heartbeat interval, disconnect)
                warn!("WebSocket pong timeout, closing");
                let _ = tx_heartbeat
                    .send(
                        ServerMessage::Error {
                            message: "pong timeout".to_string(),
                            code: 408,
                        }
                        .to_text(),
                    )
                    .await;
                break;
            }

            // Send ping (as application-level JSON ping)
            awaiting_pong_hb.store(true, Ordering::Relaxed);
            if tx_heartbeat
                .send(r#"{"type":"ping"}"#.to_string())
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Batch buffer for upserts (debounce)
    let (batch_tx, mut batch_rx) = tokio::sync::mpsc::channel::<(String, Vec<WsPointInput>)>(512);

    // Batch processor task
    let app_state_batch = app_state.clone();
    let tx_out_batch = tx_out.clone();
    let batch_handle = tokio::spawn(async move {
        let debounce = Duration::from_millis(BATCH_DEBOUNCE_MS);

        loop {
            // Wait for first message in batch
            let first = match batch_rx.recv().await {
                Some(msg) => msg,
                None => break,
            };

            let mut batch: Vec<(String, Vec<WsPointInput>)> = vec![first];

            // Accumulate for debounce period
            let deadline = time::Instant::now() + debounce;
            loop {
                let remaining = deadline.saturating_duration_since(time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match time::timeout(remaining, batch_rx.recv()).await {
                    Ok(Some(msg)) => batch.push(msg),
                    _ => break,
                }
            }

            // Process accumulated batch, grouped by collection
            let mut grouped: std::collections::HashMap<String, Vec<WsPointInput>> =
                std::collections::HashMap::new();
            for (collection, points) in batch {
                grouped.entry(collection).or_default().extend(points);
            }

            for (collection, points) in grouped {
                let result = process_upsert_batch(&app_state_batch, &collection, points).await;
                let msg = match result {
                    Ok((upserted, failed, took_ms)) => ServerMessage::Ack {
                        upserted,
                        failed,
                        took_ms,
                    },
                    Err(e) => e,
                };
                if tx_out_batch.send(msg.to_text()).await.is_err() {
                    break;
                }
            }
        }
    });

    // Main receive loop
    loop {
        match receiver.next().await {
            Some(Ok(Message::Text(text))) => {
                last_activity.store(unix_now(), Ordering::Relaxed);
                crate::metrics::WS_MESSAGES_RECEIVED_TOTAL
                    .with_label_values(&["text"])
                    .inc();

                let text_str: &str = &text;
                match serde_json::from_str::<ClientMessage>(text_str) {
                    Ok(ClientMessage::Upsert { collection, points }) => {
                        crate::metrics::WS_MESSAGES_RECEIVED_TOTAL
                            .with_label_values(&["upsert"])
                            .inc();
                        if batch_tx.send((collection, points)).await.is_err() {
                            break;
                        }
                    }
                    Ok(ClientMessage::Subscribe { collection, events }) => {
                        crate::metrics::WS_MESSAGES_RECEIVED_TOTAL
                            .with_label_values(&["subscribe"])
                            .inc();

                        if subscriptions.contains(&collection) {
                            let msg = ServerMessage::Error {
                                message: format!("already subscribed to '{collection}'"),
                                code: 409,
                            };
                            if tx_out.send(msg.to_text()).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        // Verify collection exists
                        if !app_state.collections.contains_key(&collection) {
                            let msg = ServerMessage::Error {
                                message: "collection not found".to_string(),
                                code: 404,
                            };
                            if tx_out.send(msg.to_text()).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        let event_filter: HashSet<String> = events.into_iter().collect();

                        // Subscribe to broadcast channel
                        let rx = app_state
                            .get_or_create_event_channel(&collection)
                            .subscribe();
                        subscriptions.insert(collection.clone());

                        let tx_out_sub = tx_out.clone();
                        let sub_handle = tokio::spawn(forward_events(rx, tx_out_sub, event_filter));
                        subscription_handles.push(sub_handle);

                        // Send ack for subscription
                        let msg = ServerMessage::Ack {
                            upserted: 0,
                            failed: 0,
                            took_ms: 0,
                        };
                        if tx_out.send(msg.to_text()).await.is_err() {
                            break;
                        }
                    }
                    Ok(ClientMessage::Ping) => {
                        crate::metrics::WS_MESSAGES_RECEIVED_TOTAL
                            .with_label_values(&["ping"])
                            .inc();
                        // Client ping → server pong
                        awaiting_pong.store(false, Ordering::Relaxed);
                        let msg = ServerMessage::Pong;
                        if tx_out.send(msg.to_text()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let msg = ServerMessage::Error {
                            message: format!("invalid message: {e}"),
                            code: 400,
                        };
                        if tx_out.send(msg.to_text()).await.is_err() {
                            break;
                        }
                    }
                }
            }
            Some(Ok(Message::Pong(_))) => {
                // Protocol-level pong (response to our ping)
                last_activity.store(unix_now(), Ordering::Relaxed);
                awaiting_pong.store(false, Ordering::Relaxed);
            }
            Some(Ok(Message::Ping(data))) => {
                // Protocol-level ping — axum auto-responds with pong
                last_activity.store(unix_now(), Ordering::Relaxed);
                let _ = data; // axum handles pong automatically
            }
            Some(Ok(Message::Close(_))) | None => {
                debug!("WebSocket connection closed");
                break;
            }
            Some(Ok(_)) => {
                // Binary or other — ignore
                continue;
            }
            Some(Err(e)) => {
                warn!("WebSocket error: {}", e);
                break;
            }
        }
    }

    // Cleanup
    drop(batch_tx); // Signal batch processor to stop
    heartbeat_handle.abort();
    batch_handle.abort();
    write_handle.abort();
    for h in subscription_handles {
        h.abort();
    }

    // Decrement gauge
    app_state
        .ws_connections_active
        .fetch_sub(1, Ordering::Relaxed);
    crate::metrics::WS_CONNECTIONS_ACTIVE.dec();

    info!("WebSocket connection terminated");
}

/// Processa um batch de upserts em uma coleção.
async fn process_upsert_batch(
    app_state: &AppState,
    collection_name: &str,
    points: Vec<WsPointInput>,
) -> Result<(usize, usize, u64), ServerMessage> {
    use ferres_db_core::Point;

    let start = Instant::now();

    let collection_arc =
        app_state
            .collections
            .get(collection_name)
            .ok_or_else(|| ServerMessage::Error {
                message: "collection not found".to_string(),
                code: 404,
            })?;

    let mut valid_points = Vec::new();
    let mut failed = 0usize;
    let mut point_ids = Vec::new();

    {
        let mut collection = collection_arc.write().map_err(|e| ServerMessage::Error {
            message: format!("failed to acquire write lock: {e}"),
            code: 500,
        })?;

        for input in points {
            // Validate dimension
            if collection.validate_dimension(&input.vector).is_err() {
                failed += 1;
                continue;
            }
            if input.vector.is_empty() {
                failed += 1;
                continue;
            }
            if input.vector.iter().any(|v| !v.is_finite()) {
                failed += 1;
                continue;
            }

            match Point::new(input.id.clone(), input.vector, input.metadata) {
                Ok(mut point) => {
                    if let Some(ttl) = input.ttl {
                        point.expires_at = Some(unix_now().saturating_add(ttl));
                    }
                    point_ids.push(input.id);
                    valid_points.push(point);
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }

        if !valid_points.is_empty() {
            match collection.insert_batch(valid_points) {
                Ok(result) => {
                    collection.mark_dirty();
                    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;

                    // Emit event for subscribers
                    let event = CollectionEvent {
                        collection: collection_name.to_string(),
                        action: "upsert".to_string(),
                        point_ids,
                        timestamp: unix_now(),
                    };
                    drop(collection); // Release lock before emit
                    drop(collection_arc);
                    app_state.record_ingest(unix_now(), result.inserted as u64);
                    app_state.emit_event(event);

                    return Ok((result.inserted, failed, took_ms));
                }
                Err(e) => {
                    return Err(ServerMessage::Error {
                        message: format!("batch insert error: {e}"),
                        code: 500,
                    });
                }
            }
        }
    }

    let took_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok((0, failed, took_ms))
}

/// Encaminha eventos de uma coleção para o WebSocket do cliente.
async fn forward_events(
    mut rx: broadcast::Receiver<CollectionEvent>,
    tx: tokio::sync::mpsc::Sender<String>,
    event_filter: HashSet<String>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                // Filtra por tipo de evento, se especificado
                if !event_filter.is_empty() && !event_filter.contains(&event.action) {
                    continue;
                }

                let msg = ServerMessage::Event {
                    collection: event.collection,
                    action: event.action,
                    point_ids: event.point_ids,
                    timestamp: event.timestamp,
                };
                crate::metrics::WS_MESSAGES_SENT_TOTAL
                    .with_label_values(&["event"])
                    .inc();
                if tx.send(msg.to_text()).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket subscriber lagged by {} events", n);
                // Continue receiving — some events were missed
            }
            Err(broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
}

