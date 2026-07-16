//! svc-workflow internal HTTP service.

use svc_workflow::http::{self, AppState, HttpConfig};
use svc_workflow::store::postgres::migrations;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let migrate_only = std::env::args().any(|argument| argument == "--migrate");
    let config =
        if migrate_only {
            None
        } else {
            Some(HttpConfig::from_env().map_err(|message| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
            })?)
        };
    let pool = svc_workflow::store::postgres::pool::create_pool().await;
    migrations::run(&pool).await;
    tracing::info!("migrations applied successfully");
    if migrate_only {
        return Ok(());
    }

    let config = config.expect("server configuration loaded above");
    let state = AppState::new(pool, &config);
    let app = http::router(state, &config);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "svc-workflow HTTP server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
