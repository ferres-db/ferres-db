//! # AppState — estado compartilhado da aplicação
//!
//! Contém coleções isoladas com locks individuais e configurações do servidor
//! que são compartilhadas entre todas as rotas e handlers.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Instant;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Notify};
use tracing::{info, warn};

use ferres_db_core::{
    list_restore_points, recover_collection_to_timestamp, Collection, FileStorage, ReindexJob,
    SearchResult, StorageCircuitBreaker, Wal,
};

use crate::api_keys::ApiKeyStore;
use crate::audit::AuditLogger;
use crate::cloud_settings::CloudSettingsStore;
use crate::query_log_analytics::{avg_points_per_second_10m, QueryLogCache};
use crate::query_logger::QueryLogger;
use crate::users::UserStore;

// ─── GlobalQueryStats (dashboard: queries/min, top slow, histogram) ────────

/// Capacidade máxima de eventos no buffer global (últimas ~24h de tráfego).
const GLOBAL_QUERY_EVENTS_CAP: usize = 100_000;

/// Um evento de query para estatísticas globais.
#[derive(Debug, Clone)]
pub struct GlobalQueryEvent {
    pub timestamp_secs: u64,
    pub latency_ms: u64,
    pub collection: String,
}

/// Estatísticas globais de queries (últimas 24h) para o dashboard.
///
/// Usa um channel buffered para evitar contention: `record()` é non-blocking
/// (fire-and-forget via `try_send`), e uma task de background drena o channel
/// e atualiza o buffer interno.
#[derive(Debug)]
pub struct GlobalQueryStats {
    events: RwLock<VecDeque<GlobalQueryEvent>>,
    tx: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<GlobalQueryEvent>>>,
}

impl Default for GlobalQueryStats {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalQueryStats {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(VecDeque::with_capacity(GLOBAL_QUERY_EVENTS_CAP)),
            tx: std::sync::Mutex::new(None),
        }
    }

    /// Inicia a task de background que drena o channel e atualiza o buffer.
    /// Deve ser chamado uma vez após criar o AppState dentro do runtime tokio.
    pub fn start_background_drain(self: &Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<GlobalQueryEvent>(8192);
        {
            let mut guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(tx);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(256);
            loop {
                // Wait for first event (or channel close)
                match rx.recv().await {
                    Some(ev) => batch.push(ev),
                    None => break, // channel closed (shutdown)
                }
                // Drain any additional buffered events without waiting
                while batch.len() < 1024 {
                    match rx.try_recv() {
                        Ok(ev) => batch.push(ev),
                        Err(_) => break,
                    }
                }
                // Flush batch into the VecDeque
                if let Ok(mut events) = this.events.write() {
                    for ev in batch.drain(..) {
                        events.push_back(ev);
                    }
                    while events.len() > GLOBAL_QUERY_EVENTS_CAP {
                        events.pop_front();
                    }
                } else {
                    batch.clear();
                }
            }
        });
    }

    /// Registra uma query (chamado após cada busca). Non-blocking fire-and-forget.
    pub fn record(&self, collection: &str, latency_ms: u64) {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let event = GlobalQueryEvent {
            timestamp_secs,
            latency_ms,
            collection: collection.to_string(),
        };
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.as_ref() {
            let _ = tx.try_send(event); // drop if channel full — acceptable for stats
        }
    }

    /// Retorna eventos das últimas 24 horas.
    fn events_last_24h(&self) -> Vec<GlobalQueryEvent> {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now_secs.saturating_sub(24 * 3600);
        let events = self.events.read().unwrap();
        events
            .iter()
            .filter(|e| e.timestamp_secs >= cutoff)
            .cloned()
            .collect()
    }

    /// Queries por minuto (últimas 24h): cada item é (minute_ts, count).
    pub fn queries_per_minute_24h(&self) -> Vec<(u64, u64)> {
        let events = self.events_last_24h();
        let mut buckets: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        for e in &events {
            let minute = e.timestamp_secs / 60;
            *buckets.entry(minute).or_insert(0) += 1;
        }
        let mut out: Vec<(u64, u64)> = buckets.into_iter().collect();
        out.sort_by_key(|&(k, _)| k);
        out
    }

    /// Top N queries mais lentas (collection, latency_ms, timestamp_secs).
    pub fn top_slowest(&self, n: usize) -> Vec<GlobalQueryEvent> {
        let mut events = self.events_last_24h();
        events.sort_by(|a, b| b.latency_ms.cmp(&a.latency_ms));
        events.into_iter().take(n).collect()
    }

    /// Histograma de latências: buckets (label, count). Bordas: 0, 5, 10, 25, 50, 100, 500, inf.
    pub fn latency_histogram(&self) -> Vec<(&'static str, u64)> {
        let events = self.events_last_24h();
        let labels = [
            "0-5ms",
            "5-10ms",
            "10-25ms",
            "25-50ms",
            "50-100ms",
            "100-500ms",
            "500ms+",
        ];
        let mut counts = vec![0u64; 7];
        for e in &events {
            if e.latency_ms < 5 {
                counts[0] += 1;
            } else if e.latency_ms < 10 {
                counts[1] += 1;
            } else if e.latency_ms < 25 {
                counts[2] += 1;
            } else if e.latency_ms < 50 {
                counts[3] += 1;
            } else if e.latency_ms < 100 {
                counts[4] += 1;
            } else if e.latency_ms < 500 {
                counts[5] += 1;
            } else {
                counts[6] += 1;
            }
        }
        labels.iter().copied().zip(counts).collect()
    }
}

// ─── CollectionEvent (streaming / WebSocket) ────────────────────────────

/// Evento de uma coleção, propagado para subscribers WebSocket.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionEvent {
    /// Nome da coleção.
    pub collection: String,
    /// Ação que originou o evento ("upsert" ou "delete").
    pub action: String,
    /// IDs dos pontos afetados.
    pub point_ids: Vec<String>,
    /// Timestamp UNIX (segundos).
    pub timestamp: u64,
}

/// Capacidade padrão do broadcast channel por coleção.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;

// ─── ServerConfig ──────────────────────────────────────────────────────

/// Configuração do servidor HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host para bind do servidor (ex: "0.0.0.0" ou "127.0.0.1").
    #[serde(default = "default_host")]
    pub host: String,
    /// Porta do servidor.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Caminho para armazenamento persistente do VectorDB.
    #[serde(default = "default_storage_path")]
    pub storage_path: PathBuf,
    /// Nível de log (trace, debug, info, warn, error).
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// API keys para autenticação (separadas por vírgula).
    #[serde(default)]
    pub api_keys: Option<String>,
    /// Comprimir WAL com Zstd (quando usar VectorDB com WAL). Default: false.
    #[serde(default)]
    pub wal_compression: bool,
    /// Gravar snapshots em formato binário (points.bin) em vez de JSONL. Reduz tamanho e tempo de carga. Default: false.
    #[serde(default)]
    pub binary_snapshot: bool,
    /// Isolamento físico por namespace: pontos de cada namespace em `data/collections/<name>/namespaces/<ns>/points.bin`. Default: false.
    #[serde(default)]
    pub namespace_physical_isolation: bool,
    /// Se definido, este nó inicia como réplica de leitura do endereço indicado (ex: "127.0.0.1:50051"). Experimental.
    #[serde(skip_serializing)]
    pub replica_of: Option<String>,
    /// Região AWS para backup S3 (ex: "us-east-1"). Env: FERRESDB_S3_REGION ou AWS_REGION.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_region: Option<String>,
    /// Nome do bucket S3 para upload de backups. Env: FERRESDB_S3_BUCKET.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_bucket: Option<String>,
    /// Access Key ID para S3 (opcional; pode usar AWS_ACCESS_KEY_ID). Env: FERRESDB_S3_ACCESS_KEY_ID.
    #[serde(skip_serializing)]
    pub s3_access_key_id: Option<String>,
    /// Secret Access Key para S3 (opcional; pode usar AWS_SECRET_ACCESS_KEY). Env: FERRESDB_S3_SECRET_ACCESS_KEY.
    #[serde(skip_serializing)]
    pub s3_secret_access_key: Option<String>,
    /// Caminho para modelo ONNX de Cross-Encoder (re-ranking). Requer build com feature `rerank`. Env: FERRESDB_RERANK_MODEL.
    #[serde(skip_serializing)]
    pub rerank_model_path: Option<PathBuf>,
    /// Dimensão do vetor esperada pelo modelo de rerank (default: 384). Env: FERRESDB_RERANK_DIMENSION.
    #[serde(skip_serializing)]
    pub rerank_dimension: Option<usize>,
    /// Rate limit por coleção: requisições por segundo (default: 500). Env: FERRESDB_RATE_LIMIT_PER_SECOND.
    #[serde(default = "default_rate_limit_per_second")]
    pub rate_limit_per_second: u32,
    /// Rate limit por coleção: tamanho do burst (default: 1000). Env: FERRESDB_RATE_LIMIT_BURST.
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_storage_path() -> PathBuf {
    "./data".into()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_rate_limit_per_second() -> u32 {
    2_000
}

fn default_rate_limit_burst() -> u32 {
    10_000
}

impl ServerConfig {
    /// Carrega configuração de variáveis de ambiente ou arquivo TOML.
    ///
    /// Ordem de precedência:
    /// 1. Variáveis de ambiente (HOST, PORT, STORAGE_PATH, LOG_LEVEL)
    /// 2. Arquivo config.toml (se existir)
    /// 3. Valores padrão
    /// Procura config.toml no diretório atual e em diretórios pais (para quando
    /// o servidor é iniciado de subpastas como dashboard/ ou crates/server/).
    fn find_config_path() -> Option<PathBuf> {
        let candidates = ["config.toml", "../config.toml", "../../config.toml"];
        for p in candidates {
            if std::path::Path::new(p).is_file() {
                return Some(PathBuf::from(p));
            }
        }
        None
    }

    pub fn load() -> Result<Self, ConfigError> {
        let mut config = if let Some(config_path) = Self::find_config_path() {
            let toml_str = std::fs::read_to_string(&config_path).map_err(|e| {
                ConfigError::InvalidToml(format!("failed to read {}: {}", config_path.display(), e))
            })?;
            info!(path = %config_path.display(), "loading configuration from config.toml");
            toml::from_str(&toml_str).map_err(|e| ConfigError::InvalidToml(e.to_string()))?
        } else {
            info!("using default configuration (config.toml not found in ., .. or ../..)");
            Self::default()
        };

        // Variáveis de ambiente têm precedência sobre TOML
        if let Ok(host) = std::env::var("HOST") {
            config.host = host;
        }
        if let Ok(port_str) = std::env::var("PORT") {
            config.port = port_str
                .parse()
                .map_err(|_| ConfigError::InvalidEnv("PORT must be a valid u16".to_string()))?;
        }
        if let Ok(storage_path) = std::env::var("STORAGE_PATH") {
            config.storage_path = PathBuf::from(storage_path);
        }
        if let Ok(log_level) = std::env::var("LOG_LEVEL") {
            config.log_level = log_level;
        }
        if let Ok(api_keys) = std::env::var("FERRESDB_API_KEYS") {
            config.api_keys = Some(api_keys);
        }
        if let Ok(v) = std::env::var("FERRESDB_WAL_COMPRESSION") {
            config.wal_compression = v.eq_ignore_ascii_case("true") || v == "1";
        }
        if let Ok(v) = std::env::var("FERRESDB_BINARY_SNAPSHOT") {
            config.binary_snapshot = v.eq_ignore_ascii_case("true") || v == "1";
        }
        if let Ok(v) = std::env::var("FERRESDB_NAMESPACE_PHYSICAL_ISOLATION") {
            config.namespace_physical_isolation = v.eq_ignore_ascii_case("true") || v == "1";
        }
        if let Ok(addr) = std::env::var("FERRESDB_REPLICA_OF") {
            if !addr.trim().is_empty() {
                config.replica_of = Some(addr.trim().to_string());
            }
        }
        // S3 backup (Region, Bucket, Credentials)
        if let Ok(v) = std::env::var("FERRESDB_S3_REGION").or_else(|_| std::env::var("AWS_REGION"))
        {
            if !v.trim().is_empty() {
                config.s3_region = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_S3_BUCKET") {
            if !v.trim().is_empty() {
                config.s3_bucket = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_S3_ACCESS_KEY_ID")
            .or_else(|_| std::env::var("AWS_ACCESS_KEY_ID"))
        {
            if !v.trim().is_empty() {
                config.s3_access_key_id = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_S3_SECRET_ACCESS_KEY")
            .or_else(|_| std::env::var("AWS_SECRET_ACCESS_KEY"))
        {
            if !v.trim().is_empty() {
                config.s3_secret_access_key = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_RERANK_MODEL") {
            if !v.trim().is_empty() {
                config.rerank_model_path = Some(PathBuf::from(v.trim()));
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_RERANK_DIMENSION") {
            if let Ok(d) = v.trim().parse::<usize>() {
                config.rerank_dimension = Some(d);
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_RATE_LIMIT_PER_SECOND") {
            if let Ok(n) = v.trim().parse::<u32>() {
                config.rate_limit_per_second = n;
            }
        }
        if let Ok(v) = std::env::var("FERRESDB_RATE_LIMIT_BURST") {
            if let Ok(n) = v.trim().parse::<u32>() {
                config.rate_limit_burst = n;
            }
        }

        // --replica-of <ADDR> (override env)
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--replica-of") {
            if let Some(addr) = args.get(i + 1) {
                config.replica_of = Some(addr.clone());
            }
        }

        info!(
            host = %config.host,
            port = config.port,
            storage_path = %config.storage_path.display(),
            log_level = %config.log_level,
            "server configuration loaded"
        );

        Ok(config)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            storage_path: default_storage_path(),
            log_level: default_log_level(),
            api_keys: None,
            wal_compression: false,
            binary_snapshot: false,
            namespace_physical_isolation: false,
            replica_of: None,
            s3_region: None,
            s3_bucket: None,
            s3_access_key_id: None,
            s3_secret_access_key: None,
            rerank_model_path: None,
            rerank_dimension: None,
            rate_limit_per_second: default_rate_limit_per_second(),
            rate_limit_burst: default_rate_limit_burst(),
        }
    }
}

// ─── Query profile (debug: tempo por fase) ──────────────────────────────────

/// Uma fase da execução da query (validação, busca, hydrate).
#[derive(Debug, Clone, Serialize)]
pub struct QueryPhase {
    pub name: String,
    pub duration_ms: u64,
    pub percentage: f64,
}

/// Perfil de execução de uma query (para GET /api/v1/debug/query-profile/:id).
#[derive(Debug, Clone, Serialize)]
pub struct QueryProfile {
    pub query_id: String,
    pub total_ms: u64,
    pub phases: Vec<QueryPhase>,
}

/// Capacidade máxima de perfis em memória (evição ao inserir).
pub const QUERY_PROFILES_CAP: usize = 10_000;

// ─── QueryStats ────────────────────────────────────────────────────────────

/// Estatísticas de queries para uma coleção.
///
/// `record_query` é non-blocking: incrementa an atomic counter and appends to a
/// lock-free ring buffer to avoid serialising all concurrent searches.
#[derive(Debug, Clone)]
pub struct QueryStats {
    /// Número total de queries executadas.
    pub num_queries: Arc<AtomicU64>,
    /// Histórico de latências (mantém últimas 1000 para cálculo de percentis).
    pub latencies_ms: Arc<RwLock<VecDeque<u64>>>,
}

impl Default for QueryStats {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryStats {
    /// Cria uma nova instância de QueryStats.
    pub fn new() -> Self {
        Self {
            num_queries: Arc::new(AtomicU64::new(0)),
            latencies_ms: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
        }
    }

    /// Registra uma nova query com sua latência.
    /// Uses try_write to avoid blocking concurrent searches — if the lock is
    /// currently held (e.g. by calculate_percentiles), the latency sample is
    /// simply dropped. This is acceptable for approximate stats.
    pub fn record_query(&self, latency_ms: u64) {
        self.num_queries.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut latencies) = self.latencies_ms.try_write() {
            latencies.push_back(latency_ms);
            // Mantém apenas as últimas 1000 latências
            if latencies.len() > 1000 {
                latencies.pop_front();
            }
        }
        // If try_write fails, skip this sample — stats remain approximate.
    }

    /// Calcula percentis das latências.
    pub fn calculate_percentiles(&self) -> (f64, f64, f64, f64) {
        let latencies = self.latencies_ms.read().unwrap();
        let mut sorted: Vec<u64> = latencies.iter().copied().collect();
        sorted.sort();

        if sorted.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }

        let len = sorted.len();
        let avg = sorted.iter().sum::<u64>() as f64 / len as f64;
        let p50 = sorted[len * 50 / 100];
        let p95 = sorted[len * 95 / 100];
        let p99 = sorted[len * 99 / 100];

        (avg, p50 as f64, p95 as f64, p99 as f64)
    }
}

// ─── AppState ────────────────────────────────────────────────────────────

/// Estado compartilhado da aplicação HTTP.
///
/// Contém coleções isoladas com locks individuais por coleção para melhor
/// concorrência e isolamento.
#[derive(Clone)]
pub struct AppState {
    /// Mapa de coleções com lock individual por coleção.
    pub collections: Arc<DashMap<String, Arc<RwLock<Collection>>>>,
    /// Estatísticas de queries por coleção.
    pub query_stats: Arc<DashMap<String, QueryStats>>,
    /// Estatísticas globais (últimas 24h) para dashboard.
    pub global_query_stats: Arc<GlobalQueryStats>,
    /// Logger de queries.
    pub query_logger: Arc<QueryLogger>,
    /// Cache de analytics (leitura de queries.log, TTL 1h).
    pub query_log_cache: Arc<QueryLogCache>,
    /// Perfis de queries recentes (debug), keyed by query_id.
    pub query_profiles: Arc<DashMap<String, QueryProfile>>,
    /// Configuração do servidor.
    pub config: ServerConfig,
    /// Notificador para shutdown graceful.
    shutdown_notify: Arc<Notify>,
    /// Flag indicando se o servidor está em shutdown.
    pub is_shutting_down: Arc<AtomicBool>,
    /// Instante de inicialização do servidor (para health check uptime).
    pub started_at: Arc<Instant>,
    /// Store de API keys (SQLite). Se None, apenas chaves legacy (config/env) são aceitas.
    pub api_key_store: Option<Arc<ApiKeyStore>>,
    /// Store de usuários do dashboard (SQLite). Usado para login.
    pub user_store: Option<Arc<UserStore>>,
    /// Cloud (S3) backup settings from dashboard (SQLite). Overrides config when set.
    pub cloud_settings_store: Option<Arc<CloudSettingsStore>>,
    /// Logger de auditoria (append-only JSONL, rotação diária).
    pub audit_logger: Arc<AuditLogger>,
    /// Broadcast channels para eventos de collection (streaming subscribers).
    pub event_channels: Arc<DashMap<String, broadcast::Sender<CollectionEvent>>>,
    /// Contador de conexões WebSocket ativas.
    pub ws_connections_active: Arc<AtomicU64>,
    /// Máximo de conexões WebSocket simultâneas (configurável).
    pub max_ws_connections: u64,
    /// Active and completed reindex jobs, keyed by job ID.
    pub reindex_jobs: Arc<DashMap<String, Arc<RwLock<ReindexJob>>>>,
    /// Cross-Encoder re-ranker (ONNX). None se não configurado ou build sem feature rerank.
    pub reranker: Option<Arc<dyn ferres_db_core::Reranker>>,
    /// Circuit breaker for storage I/O (disk full, repeated failures).
    pub storage_circuit_breaker: Arc<StorageCircuitBreaker>,
    /// Eventos de ingestão (timestamp_sec, points_count) para séries temporais (últimos 10 min).
    ingest_events: Arc<RwLock<Vec<(u64, u64)>>>,
    /// Semaphore to limit concurrent CPU-intensive searches (prevents thread oversubscription).
    pub search_semaphore: Arc<tokio::sync::Semaphore>,

    #[cfg(feature = "raft")]
    /// Handle do nó Raft quando feature "raft" está ativa e o nó foi inicializado.
    pub raft_handle: Option<Arc<crate::raft::RaftHandle>>,
}

impl AppState {
    /// Cria uma nova instância do AppState.
    ///
    /// Carrega coleções existentes do disco e inicializa o estado.
    /// Se `api_key_store` for fornecido, a autenticação usará as chaves do SQLite.
    /// Se `user_store` for fornecido, o login do dashboard usará usuários do SQLite.
    pub fn new(
        config: ServerConfig,
        api_key_store: Option<Arc<ApiKeyStore>>,
        user_store: Option<Arc<UserStore>>,
        cloud_settings_store: Option<Arc<CloudSettingsStore>>,
        reranker: Option<Arc<dyn ferres_db_core::Reranker>>,
    ) -> Result<Self, ferres_db_core::FerresError> {
        info!(
            storage_path = %config.storage_path.display(),
            "initializing collections"
        );

        let collections = Arc::new(DashMap::new());
        let collections_dir = config.storage_path.join("collections");

        // Cria o diretório collections se não existir
        if !collections_dir.exists() {
            std::fs::create_dir_all(&collections_dir).map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to create collections directory {}: {e}",
                    collections_dir.display()
                ))
            })?;
        }

        // Carrega coleções existentes do disco
        if collections_dir.exists() {
            let entries = std::fs::read_dir(&collections_dir).map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to read collections directory {}: {e}",
                    collections_dir.display()
                ))
            })?;

            for entry in entries {
                let entry = entry.map_err(|e| {
                    ferres_db_core::FerresError::Storage(format!(
                        "failed to read directory entry: {e}"
                    ))
                })?;
                let path = entry.path();

                if path.is_dir() {
                    match FileStorage::load_collection(&path) {
                        Ok(collection) => {
                            let name = collection.name().to_string();
                            info!(
                                collection = %name,
                                points = collection.len(),
                                "loaded collection from disk"
                            );
                            let name = collection.name().to_string();
                            collections.insert(name.clone(), Arc::new(RwLock::new(collection)));
                        }
                        Err(e) => {
                            warn!(
                                path = %path.display(),
                                error = %e,
                                "failed to load collection, skipping"
                            );
                        }
                    }
                }
            }
        }

        let query_stats = Arc::new(DashMap::new());

        // Inicializa stats para coleções existentes
        for name in collections.iter().map(|e| e.key().clone()) {
            query_stats.insert(name, QueryStats::new());
        }

        let global_query_stats = Arc::new(GlobalQueryStats::new());

        // Inicializa query logger
        let log_dir = config.storage_path.join("logs");
        let query_logger = Arc::new(QueryLogger::new(log_dir.clone()).map_err(|e| {
            ferres_db_core::FerresError::Storage(format!("failed to initialize query logger: {e}"))
        })?);

        let query_log_cache = Arc::new(QueryLogCache::new(log_dir.join("queries.log")));
        let query_profiles = Arc::new(DashMap::new());

        // Inicializa audit logger (rotação diária, append-only)
        let audit_logger = Arc::new(AuditLogger::new(log_dir.clone()).map_err(|e| {
            ferres_db_core::FerresError::Storage(format!("failed to initialize audit logger: {e}"))
        })?);

        info!(
            collections = collections.len(),
            log_dir = %log_dir.display(),
            "collections initialized"
        );

        // Inicializa broadcast channels para coleções existentes
        let event_channels: Arc<DashMap<String, broadcast::Sender<CollectionEvent>>> =
            Arc::new(DashMap::new());
        for name in collections.iter().map(|e| e.key().clone()) {
            let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
            event_channels.insert(name, tx);
        }

        Ok(Self {
            collections,
            query_stats,
            global_query_stats,
            query_logger,
            query_log_cache,
            query_profiles,
            config,
            shutdown_notify: Arc::new(Notify::new()),
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            started_at: Arc::new(Instant::now()),
            api_key_store,
            user_store,
            cloud_settings_store,
            audit_logger,
            event_channels,
            ws_connections_active: Arc::new(AtomicU64::new(0)),
            max_ws_connections: 100,
            reindex_jobs: Arc::new(DashMap::new()),
            reranker,
            storage_circuit_breaker: Arc::new(StorageCircuitBreaker::new()),
            ingest_events: Arc::new(RwLock::new(Vec::with_capacity(2000))),
            search_semaphore: Arc::new(tokio::sync::Semaphore::new(num_cpus::get())),
            #[cfg(feature = "raft")]
            raft_handle: None,
        })
    }

    /// Registra pontos inseridos para séries temporais (throughput).
    /// Uses try_write to avoid blocking if another thread holds the lock.
    pub fn record_ingest(&self, timestamp_sec: u64, points_count: u64) {
        if points_count == 0 {
            return;
        }
        let mut events = match self.ingest_events.try_write() {
            Ok(g) => g,
            Err(_) => return, // skip if contended — acceptable for metrics
        };
        events.push((timestamp_sec, points_count));
        const TEN_MIN: u64 = 10 * 60;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now.saturating_sub(TEN_MIN);
        events.retain(|(ts, _)| *ts >= cutoff);
        if events.len() > 2000 {
            let drop = events.len().saturating_sub(1500);
            events.drain(0..drop);
        }
    }

    /// Eventos de ingestão das últimas 10 minutos: (timestamp_sec, points_count).
    pub fn get_ingest_events_10m(&self) -> Vec<(u64, u64)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let cutoff = now.saturating_sub(10 * 60);
        let events = match self.ingest_events.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        events
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .copied()
            .collect()
    }

    /// Média de pontos inseridos por segundo (últimos 10 min) e throughput por minuto para gráficos.
    pub fn time_series_ingest_10m(&self) -> (f64, Vec<(u64, u64)>) {
        let events = self.get_ingest_events_10m();
        let avg_pps = avg_points_per_second_10m(&events);
        let mut buckets: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        for (ts, count) in &events {
            let minute = ts / 60;
            *buckets.entry(minute).or_insert(0) += count;
        }
        let mut throughput_per_minute: Vec<(u64, u64)> = buckets.into_iter().collect();
        throughput_per_minute.sort_by_key(|&(k, _)| k);
        (avg_pps, throughput_per_minute)
    }

    /// Retorna o notificador de shutdown.
    pub fn shutdown_notify(&self) -> Arc<Notify> {
        self.shutdown_notify.clone()
    }

    /// Marca o servidor como em shutdown.
    pub fn set_shutting_down(&self) {
        self.is_shutting_down.store(true, Ordering::Release);
    }

    /// Verifica se o servidor está em shutdown.
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::Acquire)
    }

    /// Retorna o broadcast::Sender para uma coleção, criando o canal se não existir.
    pub fn get_or_create_event_channel(
        &self,
        collection: &str,
    ) -> broadcast::Sender<CollectionEvent> {
        self.event_channels
            .entry(collection.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
                tx
            })
            .clone()
    }

    /// Emite um evento no broadcast channel de uma coleção (se houver subscribers).
    pub fn emit_event(&self, event: CollectionEvent) {
        if let Some(tx) = self.event_channels.get(&event.collection) {
            // Ignora erro se não há receivers — é normal quando não há subscribers
            let _ = tx.send(event);
        }
    }

    /// Busca em todas as coleções em paralelo.
    ///
    /// Retorna pares (nome_da_coleção, resultados). Coleções com dimensão
    /// incompatível com o vetor de consulta são ignoradas (não entram no resultado).
    pub async fn search_all_collections(
        &self,
        query: Vec<f32>,
        limit: usize,
    ) -> Vec<(String, Vec<SearchResult>)> {
        let entries: Vec<(String, Arc<RwLock<Collection>>)> = self
            .collections
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();

        let handles: Vec<_> = entries
            .into_iter()
            .map(|(name, collection_arc)| {
                let query = query.clone();
                tokio::task::spawn_blocking(move || {
                    let coll = collection_arc.read().ok()?;
                    coll.validate_dimension(&query).ok()?;
                    let raw = coll.search(&query, limit, None, None).ok()?;
                    let results: Vec<SearchResult> = raw
                        .into_iter()
                        .filter_map(|(storage_id, score)| {
                            let point = coll.get(&storage_id)?;
                            Some(SearchResult {
                                id: point.id.clone(),
                                score,
                                metadata: point.metadata.clone(),
                                vector: None,
                                namespace: point.namespace.clone(),
                            })
                        })
                        .collect();
                    Some((name, results))
                })
            })
            .collect();

        let mut out = Vec::new();
        for h in handles {
            if let Ok(Some(pair)) = h.await {
                out.push(pair);
            }
        }
        out
    }

    /// Salva todas as coleções dirty no disco.
    ///
    /// Usa `try_read()` para evitar bloquear writers (vacuum, upserts) que
    /// estejam esperando o write lock. Coleções que não puderem ser lidas
    /// serão salvas na próxima iteração (~30s).
    pub fn save_dirty_collections(&self) -> Result<(), ferres_db_core::FerresError> {
        let collections_dir = self.config.storage_path.join("collections");
        let mut saved_count = 0;
        let mut skipped_count = 0;

        for entry in self.collections.iter() {
            let name = entry.key();
            let collection_arc = entry.value();

            let collection = match collection_arc.try_read() {
                Ok(c) => c,
                Err(_) => {
                    skipped_count += 1;
                    continue;
                }
            };

            if collection.is_dirty() {
                let collection_dir = collections_dir.join(name);
                let binary = self.config.binary_snapshot;
                let ns_isolation = self.config.namespace_physical_isolation;
                self.storage_circuit_breaker.call(|| {
                    FileStorage::save_collection(&collection, &collection_dir, binary, ns_isolation)
                })?;
                collection.mark_clean();
                saved_count += 1;
                info!(collection = %name, "auto-saved collection");
            }
        }

        if saved_count > 0 {
            info!(saved = saved_count, "auto-saved dirty collections");
        }
        if skipped_count > 0 {
            info!(
                skipped = skipped_count,
                "skipped locked collections (will retry next cycle)"
            );
        }

        Ok(())
    }

    /// Salva todas as coleções no disco (usado no graceful shutdown).
    ///
    /// Usa apenas read locks — `mark_clean()` opera sobre `AtomicBool`.
    pub fn save_all_collections(&self) -> Result<(), ferres_db_core::FerresError> {
        let collections_dir = self.config.storage_path.join("collections");
        let mut saved_count = 0;

        for entry in self.collections.iter() {
            let name = entry.key();
            let collection_arc = entry.value();

            let collection_dir = collections_dir.join(name);
            let collection = collection_arc.read().map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to acquire read lock for collection {name}: {e}"
                ))
            })?;
            let binary = self.config.binary_snapshot;
            let ns_isolation = self.config.namespace_physical_isolation;
            self.storage_circuit_breaker.call(|| {
                FileStorage::save_collection(&collection, &collection_dir, binary, ns_isolation)
            })?;
            collection.mark_clean();
            saved_count += 1;
        }

        info!(saved = saved_count, "saved all collections during shutdown");
        Ok(())
    }

    /// Lista pontos de restauração (PITR) para uma coleção ou todas.
    /// Retorna mapa nome_coleção -> RestorePoints (last_snapshot_timestamp + wal_timestamps).
    pub fn list_restore_points(
        &self,
        collection_name: Option<&str>,
    ) -> Result<
        std::collections::HashMap<String, ferres_db_core::RestorePoints>,
        ferres_db_core::FerresError,
    > {
        let collections_dir = self.config.storage_path.join("collections");
        let mut out = std::collections::HashMap::new();
        if !collections_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&collections_dir)
            .map_err(|e| ferres_db_core::FerresError::Storage(format!("read dir: {e}")))?
        {
            let entry = entry.map_err(|e| {
                ferres_db_core::FerresError::Storage(format!("read dir entry: {e}"))
            })?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if collection_name.map(|n| n == name).unwrap_or(true) && path.is_dir() {
                if path.join("config.json").exists() {
                    match list_restore_points(&path) {
                        Ok(rp) => {
                            out.insert(name, rp);
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        Ok(out)
    }

    /// Point-in-Time Recovery: restaura uma coleção ao estado no timestamp dado.
    /// Carrega o último snapshot, reaplica o WAL até o timestamp, substitui a coleção em memória,
    /// persiste no disco e trunca o WAL.
    pub fn restore_collection_to_timestamp(
        &self,
        name: &str,
        target_timestamp: u64,
    ) -> Result<(), ferres_db_core::FerresError> {
        let collections_dir = self.config.storage_path.join("collections");
        let collection_dir = collections_dir.join(name);
        if !collection_dir.join("config.json").exists() {
            return Err(ferres_db_core::FerresError::CollectionNotFound(
                name.to_string(),
            ));
        }

        let collection = match self
            .storage_circuit_breaker
            .call(|| recover_collection_to_timestamp(&collection_dir, target_timestamp))?
        {
            Some(c) => c,
            None => {
                return Err(ferres_db_core::FerresError::CollectionNotFound(
                    name.to_string(),
                ))
            }
        };

        {
            let guard = self
                .collections
                .get(name)
                .ok_or_else(|| ferres_db_core::FerresError::CollectionNotFound(name.to_string()))?;
            let mut coll_guard = guard
                .write()
                .map_err(|e| ferres_db_core::FerresError::Storage(format!("lock: {e}")))?;
            *coll_guard = collection;
        }

        let guard = self
            .collections
            .get(name)
            .ok_or_else(|| ferres_db_core::FerresError::CollectionNotFound(name.to_string()))?;
        let collection = guard
            .read()
            .map_err(|e| ferres_db_core::FerresError::Storage(format!("lock: {e}")))?;
        self.storage_circuit_breaker.call(|| {
            FileStorage::save_collection(
                &collection,
                &collection_dir,
                self.config.binary_snapshot,
                self.config.namespace_physical_isolation,
            )
        })?;
        collection.mark_clean();
        drop(collection);
        drop(guard);

        let mut wal = Wal::open(
            &collection_dir,
            Wal::DEFAULT_SNAPSHOT_THRESHOLD,
            self.config.wal_compression,
        )?;
        wal.truncate_after_snapshot()?;

        info!(
            collection = %name,
            target_timestamp,
            "PITR restore completed"
        );
        Ok(())
    }
}

// ─── ConfigError ────────────────────────────────────────────────────────

/// Erro ao carregar configuração.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid TOML configuration: {0}")]
    InvalidToml(String),
    #[error("invalid environment variable: {0}")]
    InvalidEnv(String),
}
