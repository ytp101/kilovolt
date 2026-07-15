mod config;
mod budget;
mod dashboard;
mod proxy;

use axum::{
    routing::{get, post},
    Router,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::time::Instant;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::AppState;
use crate::dashboard::{get_dashboard, get_stats};
use crate::proxy::{chat_completions_proxy, mock_chat_completions};

/// Simple health check probe.
async fn health_check() -> &'static str {
    "OK"
}

/// Asynchronous background function to check for updates on telemetry server.
async fn check_for_updates(client: reqwest::Client) {
    let current_version = env!("CARGO_PKG_VERSION");
    let is_docker = std::path::Path::new("/.dockerenv").exists();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let telemetry_endpoint = std::env::var("KILOVOLT_TELEMETRY_URL")
        .unwrap_or_else(|_| "https://kilovolt.vercel.app/v1/update-check".to_string());

    let url = format!(
        "{}?version={}&is_docker={}&os={}&arch={}",
        telemetry_endpoint, current_version, is_docker, os, arch
    );

    info!("Checking for updates at {}...", telemetry_endpoint);

    match client.get(&url).timeout(std::time::Duration::from_secs(5)).send().await {
        Ok(res) => {
            if res.status().is_success() {
                #[derive(serde::Deserialize)]
                struct UpdateResponse {
                    latest_version: String,
                    update_available: bool,
                    message: Option<String>,
                }

                if let Ok(update) = res.json::<UpdateResponse>().await {
                    if update.update_available {
                        info!(
                            "A new version of Kilovolt is available: {} (current: {})",
                            update.latest_version, current_version
                        );
                        if let Some(msg) = update.message {
                            info!("Update message: {}", msg);
                        }
                    } else {
                        info!("Kilovolt is up to date (version: {})", current_version);
                    }
                }
            } else {
                info!("Telemetry update check returned status: {}", res.status());
            }
        }
        Err(e) => {
            info!("Failed to check for updates (endpoint offline or unreachable): {:?}", e);
        }
    }
}

/// Helper function to listen for SIGINT or SIGTERM signals and begin graceful draining.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received. Starting graceful connection draining...");
}

#[tokio::main]
async fn main() {
    // Load environment variables from a `.env` file if present
    dotenvy::dotenv().ok();

    // Initialize structured observability logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kilovolt=info,tower_http=debug,axum=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Kilovolt (kvlt) gateway engine...");

    // Extract dynamic environment variables with safe production fallbacks
    let port = std::env::var("KILOVOLT_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8080);

    let default_budget = std::env::var("KILOVOLT_DEFAULT_BUDGET")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.00);

    info!(
        port = %port,
        default_budget = %default_budget,
        "Configuration loaded successfully"
    );

    // Create the reqwest Client with a connection pool
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(100)
        .build()
        .expect("Failed to build reqwest client");

    // Initialize global shared in-memory spend tracker state
    let spend_tracker = Arc::new(RwLock::new(HashMap::new()));

    let state = AppState {
        client,
        spend_tracker,
        default_budget,
        port,
        start_time: Instant::now(),
        total_requests: Arc::new(AtomicUsize::new(0)),
        total_latency_ms: Arc::new(AtomicU64::new(0)),
        total_tokens_consumed: Arc::new(AtomicUsize::new(0)),
        recent_requests: Arc::new(Mutex::new(VecDeque::new())),
    };

    // Spawn background task to check for updates via the telemetry server
    let update_client = state.client.clone();
    tokio::spawn(async move {
        check_for_updates(update_client).await;
    });

    // Build the Axum Router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/dashboard", get(get_dashboard))
        .route("/api/stats", get(get_stats))
        .route("/v1/chat/completions", post(chat_completions_proxy))
        .route("/mock/v1/chat/completions", post(mock_chat_completions))
        .with_state(state);

    // Bind and serve dynamically using HOST / BIND_ADDR and KILOVOLT_PORT
    let host = std::env::var("BIND_ADDR")
        .ok()
        .or_else(|| std::env::var("HOST").ok())
        .unwrap_or_else(|| "0.0.0.0".to_string());

    let addr = if host.contains(':') {
        host
    } else {
        format!("{}:{}", host, port)
    };

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to {}: {:?}", addr, e);
            std::process::exit(1);
        }
    };
    info!("Kilovolt listening on http://{}", addr);

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!("Server error during execution: {:?}", e);
    }

    info!("Graceful connection drain complete. Server shut down cleanly.");
}
