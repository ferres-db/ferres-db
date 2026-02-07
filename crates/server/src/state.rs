//! # AppState — estado compartilhado da aplicação
//!
//! Contém coleções isoladas com locks individuais e configurações do servidor
//! que são compartilhadas entre todas as rotas e handlers.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::RwLock;
use std::collections::VecDeque;
use std::time::Instant;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Notify};
use tracing::{info, warn};

use ferres_db_core::{Collection, FileStorage, SearchResult};

use crate::api_keys::ApiKeyStore;
use crate::audit::AuditLogger;
use crate::users::UserStore;
use crate::query_logger::QueryLogger;
use crate::query_log_analytics::QueryLogCache;

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
#[derive(Debug)]
pub struct GlobalQueryStats {
    events: RwLock<VecDeque<GlobalQueryEvent>>,
}

impl GlobalQueryStats {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(VecDeque::with_capacity(GLOBAL_QUERY_EVENTS_CAP)),
        }
    }

    /// Registra uma query (chamado após cada busca).
    pub fn record(&self, collection: &str, latency_ms: u64) {
        let timestamp_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut events = self.events.write().unwrap();
        events.push_back(GlobalQueryEvent {
            timestamp_secs,
            latency_ms,
            collection: collection.to_string(),
        });
        while events.len() > GLOBAL_QUERY_EVENTS_CAP {
            events.pop_front();
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
        let labels = ["0-5ms", "5-10ms", "10-25ms", "25-50ms", "50-100ms", "100-500ms", "500ms+"];
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

impl ServerConfig {
    /// Carrega configuração de variáveis de ambiente ou arquivo TOML.
    ///
    /// Ordem de precedência:
    /// 1. Variáveis de ambiente (HOST, PORT, STORAGE_PATH, LOG_LEVEL)
    /// 2. Arquivo config.toml (se existir)
    /// 3. Valores padrão
    pub fn load() -> Result<Self, ConfigError> {
        let mut config = if let Ok(toml_str) = std::fs::read_to_string("config.toml") {
            info!("loading configuration from config.toml");
            toml::from_str(&toml_str).map_err(|e| ConfigError::InvalidToml(e.to_string()))?
        } else {
            info!("using default configuration (config.toml not found)");
            Self::default()
        };

        // Variáveis de ambiente têm precedência sobre TOML
        if let Ok(host) = std::env::var("HOST") {
            config.host = host;
        }
        if let Ok(port_str) = std::env::var("PORT") {
            config.port = port_str.parse().map_err(|_| {
                ConfigError::InvalidEnv("PORT must be a valid u16".to_string())
            })?;
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
#[derive(Debug, Clone)]
pub struct QueryStats {
    /// Número total de queries executadas.
    pub num_queries: Arc<AtomicU64>,
    /// Histórico de latências (mantém últimas 1000 para cálculo de percentis).
    pub latencies_ms: Arc<RwLock<VecDeque<u64>>>,
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
    pub fn record_query(&self, latency_ms: u64) {
        self.num_queries.fetch_add(1, Ordering::Relaxed);
        
        let mut latencies = self.latencies_ms.write().unwrap();
        latencies.push_back(latency_ms);
        // Mantém apenas as últimas 1000 latências
        if latencies.len() > 1000 {
            latencies.pop_front();
        }
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
    /// Logger de auditoria (append-only JSONL, rotação diária).
    pub audit_logger: Arc<AuditLogger>,
    /// Broadcast channels para eventos de collection (streaming subscribers).
    pub event_channels: Arc<DashMap<String, broadcast::Sender<CollectionEvent>>>,
    /// Contador de conexões WebSocket ativas.
    pub ws_connections_active: Arc<AtomicU64>,
    /// Máximo de conexões WebSocket simultâneas (configurável).
    pub max_ws_connections: u64,
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
        let query_logger = Arc::new(
            QueryLogger::new(log_dir.clone()).map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to initialize query logger: {e}"
                ))
            })?
        );

        let query_log_cache = Arc::new(QueryLogCache::new(log_dir.join("queries.log")));
        let query_profiles = Arc::new(DashMap::new());

        // Inicializa audit logger (rotação diária, append-only)
        let audit_logger = Arc::new(
            AuditLogger::new(log_dir.clone()).map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to initialize audit logger: {e}"
                ))
            })?
        );

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
            audit_logger,
            event_channels,
            ws_connections_active: Arc::new(AtomicU64::new(0)),
            max_ws_connections: 100,
        })
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
                    let raw = coll.search(&query, limit).ok()?;
                    let results: Vec<SearchResult> = raw
                        .into_iter()
                        .filter_map(|(id, score)| {
                            let point = coll.get(&id)?;
                            Some(SearchResult {
                                id,
                                score,
                                metadata: point.metadata.clone(),
                                vector: None,
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
    /// Usa apenas read locks — `is_dirty()` e `mark_clean()` operam sobre
    /// `AtomicBool` e não precisam de acesso exclusivo à coleção.
    pub fn save_dirty_collections(&self) -> Result<(), ferres_db_core::FerresError> {
        let collections_dir = self.config.storage_path.join("collections");
        let mut saved_count = 0;

        for entry in self.collections.iter() {
            let name = entry.key();
            let collection_arc = entry.value();

            let collection = collection_arc.read().map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to acquire read lock for collection {}: {e}",
                    name
                ))
            })?;

            if collection.is_dirty() {
                let collection_dir = collections_dir.join(name);
                FileStorage::save_collection(&collection, &collection_dir)?;
                collection.mark_clean();
                saved_count += 1;
                info!(collection = %name, "auto-saved collection");
            }
        }

        if saved_count > 0 {
            info!(saved = saved_count, "auto-saved dirty collections");
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
                    "failed to acquire read lock for collection {}: {e}",
                    name
                ))
            })?;
            FileStorage::save_collection(&collection, &collection_dir)?;
            collection.mark_clean();
            saved_count += 1;
        }

        info!(saved = saved_count, "saved all collections during shutdown");
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

