use std::net::SocketAddr;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Registry, Layer};
use tracing_appender::{non_blocking, rolling};
use tower_http::cors::CorsLayer;

use std::sync::Arc;
use ferres_db_server::api_keys::ApiKeyStore;
use ferres_db_server::auth;
use ferres_db_server::cloud_settings::CloudSettingsStore;
use ferres_db_server::state::{AppState, ServerConfig};
use ferres_db_server::users::UserStore;
use ferres_db_server::handlers::reindex::{run_auto_reindex_cycle, run_auto_vacuum_cycle};
use ferres_db_server::routes;
use ferres_db_server::middleware;
use ferres_db_server::metrics;

fn enable_mcp() -> bool {
    std::env::args().any(|a| a == "--mcp")
        || std::env::var("FERRESDB_ENABLE_MCP")
            .as_deref()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Carrega .env do diretório atual ou do workspace (para FERRESDB_API_KEYS, etc.)
    dotenvy::dotenv().ok();

    // MCP via STDIO: quando ativo, logs de console devem ir para stderr para não corromper o protocolo em stdout
    let use_stderr_console = enable_mcp();

    // Carrega configuração
    let config = ServerConfig::load().map_err(|e| {
        eprintln!("Failed to load configuration: {e}");
        e
    })?;

    // Configura logging estruturado com JSON output e arquivo rotativo
    let log_level = config.log_level.clone();
    let log_dir = config.storage_path.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| {
        eprintln!("Failed to create log directory: {e}");
        e
    })?;

    // Cria appender rotativo para arquivo (rota diariamente, mantém 7 dias)
    let file_appender = rolling::daily(&log_dir, "server.log");
    let (non_blocking_appender, file_guard) = non_blocking(file_appender);

    // Configura subscriber com JSON format para arquivo e texto para console
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&log_level));

    // Layer para arquivo (JSON); clone para poder usar também no branch OTel
    let file_layer = fmt::layer()
        .with_writer(non_blocking_appender.clone())
        .json()
        .with_filter(env_filter.clone());

    // Inicializa o subscriber (com layer OTel quando feature "otel" e init ok).
    // Com MCP ativo, console usa stderr para não corromper o protocolo MCP em stdout.
    #[cfg(not(feature = "otel"))]
    {
        if use_stderr_console {
            Registry::default()
                .with(file_layer)
                .with(fmt::layer().with_writer(std::io::stderr).with_filter(EnvFilter::new("info")))
                .init();
        } else {
            Registry::default()
                .with(file_layer)
                .with(fmt::layer().with_writer(std::io::stdout).with_filter(EnvFilter::new("info")))
                .init();
        }
    }

    #[cfg(feature = "otel")]
    {
        match ferres_db_server::tracing_otel::init_otel_tracing() {
            Ok((otel_layer, otel_provider)) => {
                let _otel_provider = otel_provider; // mantém vivo para exportar spans
                if use_stderr_console {
                    Registry::default()
                        .with(otel_layer)
                        .with(fmt::layer().with_writer(non_blocking_appender).json().with_filter(env_filter.clone()))
                        .with(fmt::layer().with_writer(std::io::stderr).with_filter(EnvFilter::new("info")))
                        .init();
                } else {
                    Registry::default()
                        .with(otel_layer)
                        .with(fmt::layer().with_writer(non_blocking_appender).json().with_filter(env_filter.clone()))
                        .with(fmt::layer().with_writer(std::io::stdout).with_filter(EnvFilter::new("info")))
                        .init();
                }
                info!("OpenTelemetry tracing enabled (OTLP)");
            }
            Err(e) => {
                warn!(error = %e, "OpenTelemetry init failed, continuing without OTLP export");
                if use_stderr_console {
                    Registry::default()
                        .with(file_layer)
                        .with(fmt::layer().with_writer(std::io::stderr).with_filter(EnvFilter::new("info")))
                        .init();
                } else {
                    Registry::default()
                        .with(file_layer)
                        .with(fmt::layer().with_writer(std::io::stdout).with_filter(EnvFilter::new("info")))
                        .init();
                }
            }
        }
    }

    // Mantém o guard vivo para garantir que logs sejam escritos
    // O guard precisa ser mantido durante toda a execução do programa
    // Será dropado automaticamente quando o programa terminar
    let _file_guard = file_guard;

    info!("FerresDB server starting...");

    if config.api_keys.as_ref().map_or(true, |s| s.trim().is_empty()) {
        warn!(
            "No API keys configured. Set api_keys in config.toml or FERRESDB_API_KEYS env; \
             all protected routes will return 403 Invalid API key."
        );
    }

    // Inicializa store de API keys (SQLite) e opcionalmente chaves bootstrap do config/env
    let api_keys_path = config.storage_path.join("api_keys.db");
    let api_key_store = ApiKeyStore::new(&api_keys_path).map_err(|e| {
        eprintln!("Failed to open API key store at {}: {}", api_keys_path.display(), e);
        e
    })?;
    api_key_store.init(config.api_keys.as_deref()).map_err(|e| {
        eprintln!("Failed to initialize API key store: {e}");
        e
    })?;
    let api_key_store = Some(Arc::new(api_key_store));
    info!("API key store initialized (multi-key support enabled)");

    // Chaves do config/env continuam válidas como "super" (legacy) além das do SQLite
    auth::init_api_keys_from(config.api_keys.as_deref());

    // Store de usuários do dashboard (SQLite) e usuário padrão root/ferresdb
    let users_path = config.storage_path.join("users.db");
    let user_store = UserStore::new(&users_path).map_err(|e| {
        eprintln!("Failed to open user store at {}: {}", users_path.display(), e);
        e
    })?;
    user_store.ensure_default_user().map_err(|e| {
        eprintln!("Failed to ensure default user: {e}");
        e
    })?;
    let user_store = Some(Arc::new(user_store));
    info!("User store initialized (default user root)");

    // Cloud (S3) settings from dashboard (SQLite)
    let cloud_settings_path = config.storage_path.join("cloud_settings.db");
    let cloud_settings_store = CloudSettingsStore::new(&cloud_settings_path).map_err(|e| {
        eprintln!("Failed to open cloud settings store at {}: {}", cloud_settings_path.display(), e);
        e
    })?;
    let cloud_settings_store = Some(Arc::new(cloud_settings_store));
    info!("Cloud settings store initialized");

    // JWT para sessão do dashboard (FERRESDB_JWT_SECRET ou valor padrão em dev)
    let jwt_secret = std::env::var("FERRESDB_JWT_SECRET")
        .unwrap_or_else(|_| "ferresdb-dashboard-secret-change-in-production".to_string());
    auth::set_jwt_secret(jwt_secret.into_bytes());

    // Inicializa métricas Prometheus
    // As métricas são registradas automaticamente via lazy_static no módulo metrics
    info!("Prometheus metrics initialized");

    // Inicializa AppState (com store de API keys, usuários e cloud settings)
    let app_state = AppState::new(config.clone(), api_key_store, user_store, cloud_settings_store)
        .map_err(|e| {
            error!(error = %e, "failed to initialize collections");
            e
        })?;

    // Atualiza gauge de coleções ativas
    metrics::COLLECTIONS_ACTIVE.set(app_state.collections.len() as f64);

    // Warmup: reexecuta últimas 50 queries do log em background para carregar HNSW e search_cache
    ferres_db_server::warmup::spawn_warmup_task(app_state.clone());

    // Log do status de aceleração SIMD no startup
    let simd = ferres_db_core::simd_enabled();
    if simd {
        info!("SIMD acceleration: active (AVX2 or SSE4.1)");
    } else {
        info!("SIMD acceleration: scalar fallback (no AVX2/SSE4.1 detected)");
    }

    // Inicia servidor MCP via STDIO quando --mcp ou FERRESDB_ENABLE_MCP=true (requer build com --features mcp)
    #[cfg(feature = "mcp")]
    if use_stderr_console {
        ferres_db_server::mcp::spawn_mcp_server(Arc::new(app_state.clone()));
        info!("MCP server started (STDIO)");
    }

    // Inicia background task para auto-save a cada 30 segundos
    let app_state_for_save = app_state.clone();
    let shutdown_notify = app_state.shutdown_notify();
    let shutdown_notify_for_task = shutdown_notify.clone();
    let is_shutting_down = app_state.is_shutting_down.clone();

    let save_task_handle = tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !is_shutting_down.load(std::sync::atomic::Ordering::Acquire) {
                        if let Err(e) = app_state_for_save.save_dirty_collections() {
                            warn!(error = %e, "failed to auto-save collections");
                        }
                    }
                }
                _ = shutdown_notify_for_task.notified() => {
                    info!("auto-save task shutting down");
                    break;
                }
            }
        }
    });

    // Inicia background task para auto-reindex a cada 30 minutos (fragmentação por tombstones)
    const AUTO_REINDEX_INTERVAL_SECS: u64 = 30 * 60;
    let app_state_for_reindex = app_state.clone();
    let shutdown_notify_reindex = app_state.shutdown_notify();
    let is_shutting_down_reindex = app_state.is_shutting_down.clone();
    let reindex_task_handle = tokio::spawn(async move {
        let mut reindex_interval = interval(Duration::from_secs(AUTO_REINDEX_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = reindex_interval.tick() => {
                    if !is_shutting_down_reindex.load(std::sync::atomic::Ordering::Acquire) {
                        run_auto_reindex_cycle(&app_state_for_reindex);
                    }
                }
                _ = shutdown_notify_reindex.notified() => {
                    info!("auto-reindex worker task shutting down");
                    break;
                }
            }
        }
    });

    // Inicia worker de replicação quando --replica-of <ADDR> (requer feature grpc)
    #[cfg(feature = "grpc")]
    if app_state.config.replica_of.is_some() {
        let state_for_replication = app_state.clone();
        tokio::spawn(async move {
            ferres_db_server::replication::run_replication_worker(state_for_replication).await;
        });
        info!("replication worker started (replica-of)");
    }

    // Inicia background task para vacuum de pontos expirados (TTL) a cada 60 segundos
    const AUTO_VACUUM_INTERVAL_SECS: u64 = 60;
    let app_state_for_vacuum = app_state.clone();
    let shutdown_notify_vacuum = app_state.shutdown_notify();
    let is_shutting_down_vacuum = app_state.is_shutting_down.clone();
    let vacuum_task_handle = tokio::spawn(async move {
        let mut vacuum_interval = interval(Duration::from_secs(AUTO_VACUUM_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = vacuum_interval.tick() => {
                    if !is_shutting_down_vacuum.load(std::sync::atomic::Ordering::Acquire) {
                        run_auto_vacuum_cycle(&app_state_for_vacuum);
                    }
                }
                _ = shutdown_notify_vacuum.notified() => {
                    info!("auto-vacuum worker task shutting down");
                    break;
                }
            }
        }
    });

    // Limite de body: default do Axum é 2MB; upserts com muitos pontos (vetores + metadata) podem exceder.
    const BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024; // 32 MB

    // Configura CORS: CORS_ORIGINS (vírgula) em runtime; senão aceita qualquer localhost/127.0.0.1 (qualquer porta)
    use axum::http::request::Parts as RequestParts;
    use axum::http::{Method, HeaderValue};
    let cors_origin = match std::env::var("CORS_ORIGINS") {
        Ok(s) => {
            let origins: Vec<HeaderValue> = s
                .split(',')
                .filter_map(|o| o.trim().parse::<HeaderValue>().ok())
                .collect();
            if origins.is_empty() {
                tower_http::cors::AllowOrigin::predicate(
                    |origin: &HeaderValue, _: &RequestParts| {
                        origin.to_str().map_or(false, |s| {
                            s.starts_with("http://localhost:")
                                || s.starts_with("http://127.0.0.1:")
                        })
                    },
                )
            } else {
                tower_http::cors::AllowOrigin::list(origins)
            }
        }
        Err(_) => tower_http::cors::AllowOrigin::predicate(
            |origin: &HeaderValue, _: &RequestParts| {
                origin.to_str().map_or(false, |s| {
                    s.starts_with("http://localhost:")
                        || s.starts_with("http://127.0.0.1:")
                })
            },
        ),
    };
    let cors = CorsLayer::new()
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
        .allow_credentials(true);

    // Cria o router com todas as rotas da API
    let app = routes::create_router()
        .layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            middleware::replica_write_guard,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(axum::middleware::from_fn(middleware::request_logger))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
        .with_state(app_state.clone());

    // Bind do servidor REST
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("invalid address {}:{} - {}", config.host, config.port, e))?;
    info!(address = %addr, "REST server listening");

    // Cria o listener REST
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Inicia o servidor REST com graceful shutdown
    let server = axum::serve(listener, app);

    // ── gRPC server (feature "grpc") ──────────────────────────────────
    #[cfg(feature = "grpc")]
    let grpc_handle = {
        let grpc_port: u16 = std::env::var("GRPC_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50051);
        let grpc_addr: SocketAddr = format!("{}:{}", config.host, grpc_port)
            .parse()
            .map_err(|e| format!("invalid gRPC address: {e}"))?;

        let grpc_service = ferres_db_server::grpc::FerresGrpcService::new(app_state.clone());

        info!(address = %grpc_addr, "gRPC server listening");

        tokio::spawn(async move {
            if let Err(e) = tonic::transport::Server::builder()
                .add_service(grpc_service.into_server())
                .serve(grpc_addr)
                .await
            {
                error!(error = %e, "gRPC server error");
            }
        })
    };

    // Aguarda sinal de shutdown: SIGTERM (docker stop) ou SIGINT (Ctrl+C em TTY).
    // Em Docker sem TTY, ctrl_c() pode completar logo e encerrar o processo; por isso
    // escutamos os sinais Unix explicitamente.
    #[cfg(unix)]
    let shutdown = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => { info!("received SIGTERM") }
            _ = sigint.recv() => { info!("received SIGINT") }
        }
    };
    #[cfg(not(unix))]
    let shutdown = async {
        tokio::signal::ctrl_c().await.expect("failed to listen for Ctrl+C");
    };

    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!(error = %e, "server error");
            }
        }
        _ = shutdown => {
            info!("shutdown signal received, performing graceful shutdown...");
        }
    }

    // Marca shutdown ANTES de notificar (tasks deixam de iniciar novos saves/reindex)
    app_state.set_shutting_down();

    // Notifica as tasks de background e aguarda terminarem
    shutdown_notify.notify_waiters();
    const SHUTDOWN_TASK_TIMEOUT: Duration = Duration::from_secs(10);
    match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, save_task_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "auto-save task panicked"),
        Err(_) => warn!(
            "auto-save task did not exit within {:?}, proceeding with shutdown save",
            SHUTDOWN_TASK_TIMEOUT
        ),
    }
    match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, reindex_task_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "auto-reindex worker task panicked"),
        Err(_) => warn!(
            "auto-reindex worker task did not exit within {:?}, proceeding with shutdown",
            SHUTDOWN_TASK_TIMEOUT
        ),
    }
    match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, vacuum_task_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "auto-vacuum worker task panicked"),
        Err(_) => warn!(
            "auto-vacuum worker task did not exit within {:?}, proceeding with shutdown",
            SHUTDOWN_TASK_TIMEOUT
        ),
    }

    // Encerra gRPC server
    #[cfg(feature = "grpc")]
    grpc_handle.abort();

    // Agora é seguro salvar (task já encerrou)
    if let Err(e) = app_state.save_all_collections() {
        error!(error = %e, "failed to save collections during shutdown");
    }

    // Flush final do audit logger (garante que nenhuma entry pendente é perdida)
    app_state.audit_logger.shutdown().await;

    info!("server shutdown complete");
    Ok(())
}
