use std::net::SocketAddr;
use tokio::signal;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Registry, Layer};
use tracing_appender::{non_blocking, rolling};

use ferres_db_server::state::{AppState, ServerConfig};
use ferres_db_server::routes;
use ferres_db_server::middleware;
use ferres_db_server::metrics;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Layer para arquivo (JSON)
    let file_layer = fmt::layer()
        .with_writer(non_blocking_appender)
        .json()
        .with_filter(env_filter.clone());

    // Layer para console (texto formatado, apenas info+)
    let console_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(EnvFilter::new("info"));

    // Inicializa o subscriber
    Registry::default()
        .with(file_layer)
        .with(console_layer)
        .init();

    // Mantém o guard vivo para garantir que logs sejam escritos
    // O guard precisa ser mantido durante toda a execução do programa
    // Será dropado automaticamente quando o programa terminar
    let _file_guard = file_guard;

    info!("FerresDB server starting...");

    // Inicializa métricas Prometheus
    // As métricas são registradas automaticamente via lazy_static no módulo metrics
    info!("Prometheus metrics initialized");

    // Inicializa AppState
    let app_state = AppState::new(config.clone())
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

    tokio::spawn(async move {
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

    // Cria o router com todas as rotas
    let app = routes::create_router()
        .layer(axum::middleware::from_fn(middleware::request_logger))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(app_state.clone());

    // Bind do servidor
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("invalid address {}:{} - {}", config.host, config.port, e))?;
    info!(address = %addr, "server listening");

    // Cria o listener
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Inicia o servidor com graceful shutdown
    let server = axum::serve(listener, app);

    // Aguarda sinal de shutdown (Ctrl+C ou SIGTERM)
    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!(error = %e, "server error");
            }
        }
        _ = signal::ctrl_c() => {
            info!("received shutdown signal, performing graceful shutdown...");
        }
    }

    // Marca como em shutdown
    app_state.set_shutting_down();
    
    // Notifica a background task para parar
    shutdown_notify.notify_one();

    // Salva todas as coleções antes de sair
    if let Err(e) = app_state.save_all_collections() {
        error!(error = %e, "failed to save collections during shutdown");
    }

    info!("server shutdown complete");
    Ok(())
}
