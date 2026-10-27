//! `kb-server` binary: reads the configuration from the environment, opens the store
//! and serves the router until SIGINT/SIGTERM. Logs requests (method, path, status,
//! latency, account and device ids), never bodies, query strings or tokens.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use kb_server::{Config, router};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "configuration");
            return ExitCode::FAILURE;
        }
    };
    if config.invite_tokens.is_empty() {
        tracing::warn!("KB_SERVER_INVITE_TOKENS is empty: no account can be created");
    }
    let app = match router(&config) {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "cannot start");
            return ExitCode::FAILURE;
        }
    };
    let listener = match tokio::net::TcpListener::bind(config.listen).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, "cannot listen");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(listen = %config.listen, "kb-server listening");
    // The peer address feeds the per-address limits of pairing and recovery reads.
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "server stopped with an error");
            ExitCode::FAILURE
        }
    }
}

/// SIGINT (Ctrl+C) everywhere, SIGTERM on Unix (`docker stop`).
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}
