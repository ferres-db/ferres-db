use std::net::SocketAddr;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Registry, Layer};
use tracing_appender::{non_blocking, rolling};
use tower_http::cors::CorsLayer;

use std::sync::Arc;
use ferres_db_server::api_keys::ApiKeyStore;
use ferres_db_server::auth;
use ferres_db_server::state::{AppState, ServerConfig};
use ferres_db_server::users::UserStore;
use ferres_db_server::routes;
use ferres_db_server::middleware;
use ferres_db_server::metrics;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Carrega .env do diretório atual ou do workspace (para FERRESDB_API_KEYS, etc.)
    dotenvy::dotenv().ok();

    // Carrega configuração
    let config = ServerConfig::load().map_err(|e| {
        eprintln!("Failed to load configuration: {}", e);
        e
    })?;

    // Configura logging estruturado com JSON output e arquivo rotativo
    let log_level = config.log_level.clone();
    let log_dir = config.storage_path.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| {
        eprintln!("Failed to create log directory: {}", e);
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

    // Layer para console (texto formatado, apenas info+)
    let console_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(EnvFilter::new("info"));

    // Inicializa o subscriber (com layer OTel quando feature "otel" e init ok)
    #[cfg(not(feature = "otel"))]
    Registry::default()
        .with(file_layer)
        .with(console_layer)
        .init();

    #[cfg(feature = "otel")]
    {
        match ferres_db_server::tracing_otel::init_otel_tracing() {
            Ok((otel_layer, otel_provider)) => {
                info!("OpenTelemetry tracing enabled (OTLP)");
                let _otel_provider = otel_provider; // mantém vivo para exportar spans
                Registry::default()
                    .with(otel_layer)
                    .with(fmt::layer().with_writer(non_blocking_appender).json().with_filter(env_filter.clone()))
                    .with(fmt::layer().with_writer(std::io::stdout).with_filter(EnvFilter::new("info")))
                    .init();
            }
            Err(e) => {
                warn!(error = %e, "OpenTelemetry init failed, continuing without OTLP export");
                Registry::default()
                    .with(file_layer)
                    .with(console_layer)
                    .init();
            }
        }
    }

    // Mantém o guard vivo para garantir que logs sejam escritos
    // O guard precisa ser mantido durante toda a execução do programa
    // Será dropado automaticamente quando o programa terminar
    let _file_guard = file_guard;

    info!("FerresDB server starting...");

    // Inicializa store de API keys (SQLite) e opcionalmente chaves bootstrap do config/env
    let api_keys_path = config.storage_path.join("api_keys.db");
    let api_key_store = ApiKeyStore::new(&api_keys_path).map_err(|e| {
        eprintln!("Failed to open API key store at {}: {}", api_keys_path.display(), e);
        e
    })?;
    api_key_store.init(config.api_keys.as_deref()).map_err(|e| {
        eprintln!("Failed to initialize API key store: {}", e);
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
        eprintln!("Failed to ensure default user: {}", e);
        e
    })?;
    let user_store = Some(Arc::new(user_store));
    info!("User store initialized (default user root)");

    // JWT para sessão do dashboard (FERRESDB_JWT_SECRET ou valor padrão em dev)
    let jwt_secret = std::env::var("FERRESDB_JWT_SECRET")
        .unwrap_or_else(|_| "ferresdb-dashboard-secret-change-in-production".to_string());
    auth::set_jwt_secret(jwt_secret.into_bytes());

    // Inicializa métricas Prometheus
    // As métricas são registradas automaticamente via lazy_static no módulo metrics
    info!("Prometheus metrics initialized");

    // Inicializa AppState (com store de API keys e de usuários)
    let app_state = AppState::new(config.clone(), api_key_store, user_store)
        .map_err(|e| {
            error!(error = %e, "failed to initialize collections");
            e
        })?;

    // Atualiza gauge de coleções ativas
    metrics::COLLECTIONS_ACTIVE.set(app_state.collections.len() as f64);

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

    // Limite de body: default do Axum é 2MB; upserts com muitos pontos (vetores + metadata) podem exceder.
    const BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024; // 32 MB

    // Configura CORS: CORS_ORIGINS (vírgula) em runtime, senão defaults (localhost)
    use axum::http::{Method, HeaderValue};
    let default_origins: Vec<HeaderValue> = [
        "http://localhost:3000",
        "http://localhost:5173",
        "http://127.0.0.1:3000",
        "http://127.0.0.1:5173",
    ]
    .iter()
    .map(|s| s.parse().unwrap())
    .collect();
    let cors_origins: Vec<HeaderValue> = std::env::var("CORS_ORIGINS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|o| o.trim().parse::<HeaderValue>().ok())
                .collect()
        })
        .filter(|v: &Vec<_>| !v.is_empty())
        .unwrap_or(default_origins);
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::list(cors_origins))
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
        ])
        .allow_credentials(true);

    // Cria o router com todas as rotas da API
    let app = routes::create_router()
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

    // Marca shutdown ANTES de notificar (task deixa de iniciar novos saves)
    app_state.set_shutting_down();

    // Notifica a task e aguarda ela terminar para evitar salvar em paralelo
    shutdown_notify.notify_one();
    const SHUTDOWN_TASK_TIMEOUT: Duration = Duration::from_secs(10);
    match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, save_task_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "auto-save task panicked"),
        Err(_) => warn!(
            "auto-save task did not exit within {:?}, proceeding with shutdown save",
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

    info!("server shutdown complete");
    Ok(())
}
