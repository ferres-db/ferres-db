//! # AppState — estado compartilhado da aplicação
//!
//! Contém coleções isoladas com locks individuais e configurações do servidor
//! que são compartilhadas entre todas as rotas e handlers.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::RwLock;
use std::collections::VecDeque;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{info, warn};

use ferres_db_core::{Collection, FileStorage};

use crate::query_logger::QueryLogger;

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
        }
    }
}

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
    /// Logger de queries.
    pub query_logger: Arc<QueryLogger>,
    /// Configuração do servidor.
    pub config: ServerConfig,
    /// Notificador para shutdown graceful.
    shutdown_notify: Arc<Notify>,
    /// Flag indicando se o servidor está em shutdown.
    pub is_shutting_down: Arc<AtomicBool>,
}

impl AppState {
    /// Cria uma nova instância do AppState.
    ///
    /// Carrega coleções existentes do disco e inicializa o estado.
    pub fn new(config: ServerConfig) -> Result<Self, ferres_db_core::FerresError> {
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

        // Inicializa query logger
        let log_dir = config.storage_path.join("logs");
        let query_logger = Arc::new(
            QueryLogger::new(log_dir.clone()).map_err(|e| {
                ferres_db_core::FerresError::Storage(format!(
                    "failed to initialize query logger: {e}"
                ))
            })?
        );

        info!(
            collections = collections.len(),
            log_dir = %log_dir.display(),
            "collections initialized"
        );

        Ok(Self {
            collections,
            query_stats,
            query_logger,
            config,
            shutdown_notify: Arc::new(Notify::new()),
            is_shutting_down: Arc::new(AtomicBool::new(false)),
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

