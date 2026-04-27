use std::net::SocketAddr;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use ferres_db_server::bootstrap::{
    bootstrap_state, build_app, init_tracing, spawn_hnsw_autotune_worker,
};
use ferres_db_server::handlers::reindex::{
    run_auto_reindex_cycle, run_auto_vacuum_cycle, run_retention_cycle,
};
use ferres_db_server::state::ServerConfig;

fn enable_mcp() -> bool {
    std::env::args().any(|a| a == "--mcp")
        || std::env::var("FERRESDB_ENABLE_MCP")
            .as_deref()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
}

/// Stack size for runtime threads (workers and blocking pool). Larger than default to avoid
/// STATUS_STACK_BUFFER_OVERRUN when many concurrent searches run in spawn_blocking (HNSW + tracing).
/// 8 MiB gives comfortable headroom for deep HNSW traversal + tracing frames in release builds.
const RUNTIME_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

/// Maximum number of threads in the blocking pool. Prevents unbounded thread
/// creation under high-concurrency `spawn_blocking` (HNSW search). When all
/// threads are busy, new blocking tasks queue until a thread becomes available,
/// which provides natural back-pressure instead of spawning thousands of threads.
const MAX_BLOCKING_THREADS: usize = 128;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(RUNTIME_THREAD_STACK_SIZE)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .enable_all()
        .build()?;
    rt.block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env from current directory or workspace (for FERRESDB_API_KEYS, etc.)
    dotenvy::dotenv().ok();

    let config = ServerConfig::load().map_err(|e| {
        eprintln!("Failed to load configuration: {e}");
        e
    })?;

    init_tracing(&config);
    info!("FerresDB server starting...");

    // MCP via STDIO: when active, console logs must go to stderr to avoid corrupting the
    // protocol on stdout. The flag is also read inside init_tracing, but we need it here
    // for the feature-gated MCP spawn block.
    let use_stderr_console = enable_mcp();

    let app_state = bootstrap_state(&config).map_err(|e| {
        error!(error = %e, "failed to bootstrap server state");
        e
    })?;

    // MCP via STDIO (feature-gated, must be started before REST server)
    #[cfg(feature = "mcp")]
    if use_stderr_console {
        ferres_db_server::mcp::spawn_mcp_server(std::sync::Arc::new(app_state.clone()));
        info!("MCP server started (STDIO)");
    }

    // Suppress unused-variable warning when mcp feature is not enabled.
    #[cfg(not(feature = "mcp"))]
    let _ = use_stderr_console;

    // Background workers — retain JoinHandles for the graceful shutdown sequence.
    let shutdown_notify = app_state.shutdown_notify();

    // Auto-save: persist dirty collections every 30 seconds.
    let app_state_for_save = app_state.clone();
    let shutdown_notify_for_task = shutdown_notify.clone();
    let is_shutting_down = app_state.is_shutting_down.clone();
    let save_task_handle = tokio::spawn(async move {
        let mut save_interval = interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = save_interval.tick() => {
                    if !is_shutting_down.load(std::sync::atomic::Ordering::Acquire) {
                        let state = app_state_for_save.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Err(e) = state.save_dirty_collections() {
                                warn!(error = %e, "failed to auto-save collections");
                            }
                        })
                        .await;
                    }
                }
                _ = shutdown_notify_for_task.notified() => {
                    info!("auto-save task shutting down");
                    break;
                }
            }
        }
    });

    // Auto-reindex: compact tombstones every 30 minutes.
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

    // Auto-vacuum: expire TTL points every 60 seconds.
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
                        let state = app_state_for_vacuum.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            run_auto_vacuum_cycle(&state);
                        })
                        .await;
                    }
                }
                _ = shutdown_notify_vacuum.notified() => {
                    info!("auto-vacuum worker task shutting down");
                    break;
                }
            }
        }
    });

    // Retention: WAL compaction by retention_days, every hour.
    const RETENTION_INTERVAL_SECS: u64 = 3600;
    let app_state_retention = app_state.clone();
    let shutdown_notify_retention = app_state.shutdown_notify();
    let is_shutting_down_retention = app_state.is_shutting_down.clone();
    let retention_task_handle = tokio::spawn(async move {
        let mut retention_interval = interval(Duration::from_secs(RETENTION_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = retention_interval.tick() => {
                    if !is_shutting_down_retention.load(std::sync::atomic::Ordering::Acquire) {
                        let state = app_state_retention.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            run_retention_cycle(&state);
                        })
                        .await;
                    }
                }
                _ = shutdown_notify_retention.notified() => {
                    info!("retention worker task shutting down");
                    break;
                }
            }
        }
    });

    // HNSW auto-tune: shutdown handled internally via notify; no JoinHandle needed.
    let _autotune = spawn_hnsw_autotune_worker(app_state.clone());

    // gRPC server (feature-gated).
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

    // Replication worker (feature-gated, only when replica_of is set).
    #[cfg(feature = "grpc")]
    if app_state.config.replica_of.is_some() {
        let state_for_replication = app_state.clone();
        tokio::spawn(async move {
            ferres_db_server::replication::run_replication_worker(state_for_replication.into())
                .await;
        });
        info!("replication worker started (replica-of)");
    }

    let app = build_app(&config, app_state.clone());

    // Bind REST server.
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| format!("invalid address {}:{} - {}", config.host, config.port, e))?;
    info!(address = %addr, "REST server listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Signal handling: SIGTERM (docker stop) or SIGINT (Ctrl+C).
    // On Docker without a TTY, ctrl_c() may complete immediately; listen to Unix signals explicitly.
    #[cfg(unix)]
    let shutdown_signal = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => { info!("received SIGTERM") }
            _ = sigint.recv() => { info!("received SIGINT") }
        }
    };
    #[cfg(not(unix))]
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl+C");
    };

    // Serve with graceful shutdown: wait for the signal, then drain in-flight requests.
    // A 30-second outer timeout prevents hanging indefinitely if clients don't close.
    const IN_FLIGHT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
    let graceful = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal);
    match tokio::time::timeout(IN_FLIGHT_DRAIN_TIMEOUT, graceful).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "server error during shutdown"),
        Err(_) => warn!(
            "server did not drain in-flight requests within {}s, proceeding",
            IN_FLIGHT_DRAIN_TIMEOUT.as_secs()
        ),
    }
    info!("shutdown signal received, performing graceful shutdown...");

    // Mark shutdown before notifying (tasks stop initiating new saves/reindexes).
    app_state.set_shutting_down();

    // Signal background workers and wait for them to exit.
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
    match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, retention_task_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => error!(error = %e, "retention worker task panicked"),
        Err(_) => warn!(
            "retention worker task did not exit within {:?}, proceeding with shutdown",
            SHUTDOWN_TASK_TIMEOUT
        ),
    }

    // Abort gRPC server (it owns its own listener; abort is the cleanest shutdown).
    #[cfg(feature = "grpc")]
    grpc_handle.abort();

    // Final save (background save task has already exited; this is safe).
    if let Err(e) = app_state.save_all_collections() {
        error!(error = %e, "failed to save collections during shutdown");
    }

    // Flush audit log — ensures no pending entries are lost.
    app_state.audit_logger.shutdown().await;

    info!("server shutdown complete");
    Ok(())
}
