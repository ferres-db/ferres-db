use std::sync::{Arc, OnceLock};

use axum::http::request::Parts as RequestParts;
use axum::http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer, Registry,
};

use crate::api_keys::ApiKeyStore;
use crate::cloud_settings::CloudSettingsStore;
use crate::llm_credentials::LlmCredentialsStore;
use crate::state::{AppState, ServerConfig};
use crate::users::UserStore;

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("API key store error: {0}")]
    ApiKeyStore(#[from] crate::api_keys::ApiKeyError),
    #[error("user store error: {0}")]
    UserStore(#[from] crate::users::UserError),
    #[error("cloud settings store error: {0}")]
    CloudSettings(#[from] crate::cloud_settings::CloudSettingsError),
    #[error("LLM credentials store error: {0}")]
    LlmCredentials(#[from] crate::llm_credentials::LlmCredentialsError),
    #[error("state initialization error: {0}")]
    StateInit(#[from] ferres_db_core::FerresError),
}

static TRACING_INIT: OnceLock<()> = OnceLock::new();

pub fn init_tracing(config: &ServerConfig) {
    TRACING_INIT.get_or_init(|| {
        let use_stderr_console = std::env::args().any(|a| a == "--mcp")
            || std::env::var("FERRESDB_ENABLE_MCP")
                .as_deref()
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);

        let log_level = config.log_level.clone();
        let log_dir = config.storage_path.join("logs");
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("warning: failed to create log directory {}: {e}", log_dir.display());
        }

        let file_appender = rolling::daily(&log_dir, "server.log");
        let (non_blocking_appender, file_guard) = non_blocking(file_appender);

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&log_level));

        let (stdout_non_blocking, stdout_guard) = non_blocking(std::io::stdout());
        let (stderr_non_blocking, stderr_guard) = non_blocking(std::io::stderr());

        #[cfg(not(feature = "otel"))]
        {
            if use_stderr_console {
                Registry::default()
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_appender)
                            .json()
                            .with_filter(env_filter.clone()),
                    )
                    .with(
                        fmt::layer()
                            .with_writer(stderr_non_blocking)
                            .with_filter(EnvFilter::new("info")),
                    )
                    .try_init()
                    .ok();
                Box::leak(Box::new(stderr_guard));
                drop(stdout_guard);
            } else {
                Registry::default()
                    .with(
                        fmt::layer()
                            .with_writer(non_blocking_appender)
                            .json()
                            .with_filter(env_filter.clone()),
                    )
                    .with(
                        fmt::layer()
                            .with_writer(stdout_non_blocking)
                            .with_filter(EnvFilter::new("info")),
                    )
                    .try_init()
                    .ok();
                Box::leak(Box::new(stdout_guard));
                drop(stderr_guard);
            }
        }

        #[cfg(feature = "otel")]
        {
            match ferres_db_server::tracing_otel::init_otel_tracing() {
                Ok((otel_layer, otel_provider)) => {
                    Box::leak(Box::new(otel_provider));
                    if use_stderr_console {
                        Registry::default()
                            .with(otel_layer)
                            .with(
                                fmt::layer()
                                    .with_writer(non_blocking_appender)
                                    .json()
                                    .with_filter(env_filter.clone()),
                            )
                            .with(
                                fmt::layer()
                                    .with_writer(stderr_non_blocking)
                                    .with_filter(EnvFilter::new("info")),
                            )
                            .try_init()
                            .ok();
                        Box::leak(Box::new(stderr_guard));
                        drop(stdout_guard);
                    } else {
                        Registry::default()
                            .with(otel_layer)
                            .with(
                                fmt::layer()
                                    .with_writer(non_blocking_appender)
                                    .json()
                                    .with_filter(env_filter.clone()),
                            )
                            .with(
                                fmt::layer()
                                    .with_writer(stdout_non_blocking)
                                    .with_filter(EnvFilter::new("info")),
                            )
                            .try_init()
                            .ok();
                        Box::leak(Box::new(stdout_guard));
                        drop(stderr_guard);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "OpenTelemetry init failed, continuing without OTLP export");
                    if use_stderr_console {
                        Registry::default()
                            .with(
                                fmt::layer()
                                    .with_writer(non_blocking_appender)
                                    .json()
                                    .with_filter(env_filter.clone()),
                            )
                            .with(
                                fmt::layer()
                                    .with_writer(stderr_non_blocking)
                                    .with_filter(EnvFilter::new("info")),
                            )
                            .try_init()
                            .ok();
                        Box::leak(Box::new(stderr_guard));
                        drop(stdout_guard);
                    } else {
                        Registry::default()
                            .with(
                                fmt::layer()
                                    .with_writer(non_blocking_appender)
                                    .json()
                                    .with_filter(env_filter.clone()),
                            )
                            .with(
                                fmt::layer()
                                    .with_writer(stdout_non_blocking)
                                    .with_filter(EnvFilter::new("info")),
                            )
                            .try_init()
                            .ok();
                        Box::leak(Box::new(stdout_guard));
                        drop(stderr_guard);
                    }
                }
            }
        }

        Box::leak(Box::new(file_guard));
    });
}

pub fn build_cors_layer(_config: &ServerConfig) -> CorsLayer {
    let cors_origin = match std::env::var("CORS_ORIGINS") {
        Ok(s) => {
            let origins: Vec<HeaderValue> = s
                .split(',')
                .filter_map(|o| o.trim().parse::<HeaderValue>().ok())
                .collect();
            if origins.is_empty() {
                tower_http::cors::AllowOrigin::predicate(
                    |origin: &HeaderValue, _: &RequestParts| {
                        origin.to_str().is_ok_and(|s| {
                            s.starts_with("http://localhost:") || s.starts_with("http://127.0.0.1:")
                        })
                    },
                )
            } else {
                tower_http::cors::AllowOrigin::list(origins)
            }
        }
        Err(_) => {
            tower_http::cors::AllowOrigin::predicate(|origin: &HeaderValue, _: &RequestParts| {
                origin.to_str().is_ok_and(|s| {
                    s.starts_with("http://localhost:") || s.starts_with("http://127.0.0.1:")
                })
            })
        }
    };

    CorsLayer::new()
        .allow_origin(cors_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::ACCEPT,
            axum::http::header::HeaderName::from_static("x-requested-with"),
        ])
        .allow_credentials(true)
}

pub fn bootstrap_state(config: &ServerConfig) -> Result<AppState, BootstrapError> {
    if config
        .api_keys
        .as_deref()
        .map_or(true, |s| s.trim().is_empty())
    {
        tracing::warn!(
            "No API keys configured. Set api_keys in config.toml or FERRESDB_API_KEYS env; \
             all protected routes will return 403 Invalid API key."
        );
    }

    let api_key_store = ApiKeyStore::new(&config.storage_path.join("api_keys.db"))?;
    api_key_store.init(config.api_keys.as_deref())?;
    let api_key_store = Arc::new(api_key_store);
    crate::api_keys::set_global_store(Some(api_key_store.clone()));
    crate::auth::init_api_keys_from(config.api_keys.as_deref());

    let user_store = UserStore::new(&config.storage_path.join("users.db"))?;
    user_store.ensure_default_user()?;

    let cloud_settings_store =
        CloudSettingsStore::new(&config.storage_path.join("cloud_settings.db"))?;

    let llm_credentials_store =
        LlmCredentialsStore::new(&config.storage_path.join("llm_credentials.db"))?;

    let jwt_secret = std::env::var("FERRESDB_JWT_SECRET")
        .unwrap_or_else(|_| "ferresdb-dashboard-secret-change-in-production".to_string());
    crate::auth::set_jwt_secret(jwt_secret.into_bytes());

    #[cfg(feature = "rerank")]
    let reranker: Option<Arc<dyn ferres_db_core::Reranker>> = match &config.rerank_model_path {
        Some(path) => {
            let dim = config.rerank_dimension.unwrap_or(384);
            match ferres_db_core::CrossEncoderOrt::load(path, dim, None, None) {
                Ok(r) => {
                    tracing::info!(path = %path.display(), dimension = dim, "cross-encoder reranker loaded");
                    Some(Arc::new(r) as Arc<dyn ferres_db_core::Reranker>)
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to load reranker, continuing without");
                    None
                }
            }
        }
        None => None,
    };
    #[cfg(not(feature = "rerank"))]
    let reranker: Option<Arc<dyn ferres_db_core::Reranker>> = None;

    let app_state = AppState::new(
        config.clone(),
        Some(api_key_store),
        Some(Arc::new(user_store)),
        Some(Arc::new(cloud_settings_store)),
        Some(Arc::new(llm_credentials_store)),
        reranker,
    )?;

    crate::metrics::COLLECTIONS_ACTIVE.set(app_state.collections.len() as f64);
    app_state.global_query_stats.start_background_drain();

    let simd = ferres_db_core::simd_enabled();
    if simd {
        tracing::info!("SIMD acceleration: active (AVX2 or SSE4.1)");
    } else {
        tracing::info!("SIMD acceleration: scalar fallback (no AVX2/SSE4.1 detected)");
    }

    Ok(app_state)
}

pub fn build_app(config: &ServerConfig, state: AppState) -> axum::Router {
    const BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
    let cors = build_cors_layer(config);
    crate::routes::create_router(config)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::replica_write_guard,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(axum::middleware::from_fn(crate::middleware::request_logger))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

pub fn spawn_hnsw_autotune_worker(state: AppState) -> tokio::task::JoinHandle<()> {
    const HNSW_AUTO_TUNE_INTERVAL_SECS: u64 = 60;
    let is_shutting_down = state.is_shutting_down.clone();
    let shutdown_notify = state.shutdown_notify();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
            HNSW_AUTO_TUNE_INTERVAL_SECS,
        ));
        loop {
            tokio::select! {
                biased;
                _ = shutdown_notify.notified() => {
                    tracing::info!("hnsw auto-tune worker task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if !is_shutting_down.load(std::sync::atomic::Ordering::Acquire) {
                        let state = state.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::handlers::stats::run_hnsw_auto_tune_cycle(&state);
                        })
                        .await;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ServerConfig;
    use serial_test::serial;

    #[test]
    #[serial]
    fn build_cors_layer_default_allows_localhost() {
        std::env::remove_var("CORS_ORIGINS");
        let config = ServerConfig::default();
        let _layer = build_cors_layer(&config);
        // Construction succeeds — behavior verified via HTTP tests
    }

    #[test]
    #[serial]
    fn build_cors_layer_with_explicit_origins() {
        std::env::set_var("CORS_ORIGINS", "http://example.com,http://app.example.com");
        let config = ServerConfig::default();
        let _layer = build_cors_layer(&config);
        std::env::remove_var("CORS_ORIGINS");
    }

    #[test]
    #[serial]
    fn build_cors_layer_empty_origins_falls_back_to_predicate() {
        std::env::set_var("CORS_ORIGINS", "   ");
        let config = ServerConfig::default();
        let _layer = build_cors_layer(&config);
        std::env::remove_var("CORS_ORIGINS");
    }

    // tokio::test needed: start_background_drain() calls tokio::spawn internally
    #[tokio::test]
    async fn bootstrap_state_empty_dir_creates_all_stores() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = ServerConfig {
            storage_path: temp_dir.path().to_path_buf(),
            api_keys: Some("test-bootstrap-key".to_string()),
            ..Default::default()
        };
        let state = bootstrap_state(&config).unwrap();
        assert!(state.api_key_store.is_some());
        assert!(state.user_store.is_some());
        assert!(state.cloud_settings_store.is_some());
        assert!(state.llm_credentials_store.is_some());
        assert!(state.collections.is_empty());
    }

    // tokio::test needed: start_background_drain() calls tokio::spawn internally
    #[tokio::test]
    async fn bootstrap_state_existing_dir_loads_state() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = ServerConfig {
            storage_path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        // First bootstrap
        let _state = bootstrap_state(&config).unwrap();
        drop(_state);
        // Second bootstrap on same dir
        let state = bootstrap_state(&config).unwrap();
        assert!(state.api_key_store.is_some());
        assert!(state.user_store.is_some());
    }
}
