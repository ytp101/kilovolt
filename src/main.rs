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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use sha2::{Sha256, Digest};

use crate::config::AppState;
use crate::dashboard::{get_dashboard, get_stats};
use crate::proxy::{chat_completions_proxy, mock_chat_completions};

/// Simple health check probe.
async fn health_check() -> &'static str {
    "OK"
}

/// Helper function to retrieve or generate a persistent anonymous client hash.
fn get_or_create_client_hash() -> String {
    let local_path = std::path::Path::new(".client_hash");
    if let Ok(content) = std::fs::read_to_string(local_path) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let tmp_path = std::path::Path::new("/tmp/kilovolt_client_hash");
    if let Ok(content) = std::fs::read_to_string(tmp_path) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    // Generate a fresh unique SHA-256 hash using a new UUID
    let new_uuid = uuid::Uuid::new_v4().to_string();
    let mut hasher = Sha256::new();
    hasher.update(new_uuid.as_bytes());
    let new_hash = format!("{:x}", hasher.finalize());

    // Persist hash (ignoring file write errors on read-only docker file systems)
    let _ = std::fs::write(local_path, &new_hash);
    let _ = std::fs::write(tmp_path, &new_hash);

    new_hash
}

/// One-time startup check-in telemetry payload sender.
async fn send_startup_telemetry(client: reqwest::Client, client_hash: String) {
    let current_version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS.to_string();
    
    // Normalize CPU architectures
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }.to_string();

    let telemetry_endpoint = std::env::var("KILOVOLT_TELEMETRY_URL")
        .unwrap_or_else(|_| "https://kilovolt.vercel.app/v1/update-check".to_string());

    info!("Sending startup telemetry check-in to {}...", telemetry_endpoint);

    let payload = serde_json::json!({
        "type": "startup",
        "client_hash": client_hash,
        "version": current_version,
        "os": os,
        "arch": arch
    });

    match client.post(&telemetry_endpoint)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
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
                    } else {
                        info!("Kilovolt is up to date (version: {})", current_version);
                    }
                    if let Some(msg) = update.message {
                        info!("Telemetry response: {}", msg);
                    }
                }
            } else {
                info!("Telemetry startup check-in returned status: {}", res.status());
            }
        }
        Err(e) => {
            info!("Failed to complete startup telemetry check-in (endpoint unreachable): {:?}", e);
        }
    }
}

/// 24-hour loop for running daily MAPD telemetry reports.
async fn run_daily_telemetry_loop(state: AppState) {
    let client = state.client.clone();
    let client_hash = state.client_hash.clone();

    loop {
        // Sleep for a full 24-hour cycle
        tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;

        let current_version = env!("CARGO_PKG_VERSION");
        let total_requests = state.total_requests.load(Ordering::Relaxed);
        let total_tokens = state.total_tokens_consumed.load(Ordering::Relaxed);

        let total_users = {
            let map = state.spend_tracker.read().unwrap();
            map.len()
        };

        let model_distribution = {
            let counts = state.model_counts.read().unwrap();
            counts.clone()
        };

        let telemetry_endpoint = std::env::var("KILOVOLT_TELEMETRY_URL")
            .unwrap_or_else(|_| "https://kilovolt.vercel.app/v1/update-check".to_string());

        info!("Sending 24hr cycle MAPD telemetry check-in to {}...", telemetry_endpoint);

        let payload = serde_json::json!({
            "type": "daily_mapd",
            "client_hash": client_hash,
            "version": current_version,
            "total_requests": total_requests,
            "total_tokens": total_tokens,
            "total_users": total_users,
            "model_distribution": model_distribution
        });

        let _ = client.post(&telemetry_endpoint)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
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
    
    // Retrieve or create client identity hash
    let client_hash = get_or_create_client_hash();
    let model_counts = Arc::new(RwLock::new(HashMap::new()));

    let state = AppState {
        client: client.clone(),
        spend_tracker,
        default_budget,
        port,
        start_time: Instant::now(),
        total_requests: Arc::new(AtomicUsize::new(0)),
        total_latency_ms: Arc::new(AtomicU64::new(0)),
        total_tokens_consumed: Arc::new(AtomicUsize::new(0)),
        recent_requests: Arc::new(Mutex::new(VecDeque::new())),
        client_hash: client_hash.clone(),
        model_counts,
    };

    // Spawn startup check-in task
    let startup_client = client.clone();
    let startup_hash = client_hash.clone();
    tokio::spawn(async move {
        send_startup_telemetry(startup_client, startup_hash).await;
    });

    // Spawn 24h cycle metrics loop
    let daily_state = state.clone();
    tokio::spawn(async move {
        run_daily_telemetry_loop(daily_state).await;
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
