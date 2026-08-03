mod budget;
mod config;
mod dashboard;
mod ledger;
mod pricing;
mod proxy;

use axum::{
    Router,
    routing::{get, post},
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::{
    AppState, DEFAULT_MAX_REQUEST_BODY_BYTES, DEFAULT_MAX_SSE_FRAME_BYTES,
    DEFAULT_MAX_UPSTREAM_BODY_BYTES, DEFAULT_TELEMETRY_URL, TelemetryConfig,
};
use crate::dashboard::{get_dashboard, get_stats};
use crate::ledger::BudgetLedger;
use crate::pricing::PricingRegistry;
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
async fn send_startup_telemetry(
    client: reqwest::Client,
    client_hash: String,
    telemetry_endpoint: String,
) {
    let current_version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS.to_string();

    // Normalize CPU architectures
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
    .to_string();

    info!(
        "Sending startup telemetry check-in to {}...",
        telemetry_endpoint
    );

    let is_docker = std::path::Path::new("/.dockerenv").exists();

    let payload = startup_telemetry_payload(&client_hash, current_version, is_docker, &os, &arch);

    match client
        .post(&telemetry_endpoint)
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
                info!(
                    "Telemetry startup check-in returned status: {}",
                    res.status()
                );
            }
        }
        Err(e) => {
            info!(
                "Failed to complete startup telemetry check-in (endpoint unreachable): {:?}",
                e
            );
        }
    }
}

fn startup_telemetry_payload(
    client_hash: &str,
    version: &str,
    is_docker: bool,
    os: &str,
    arch: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": "startup",
        "client_hash": client_hash,
        "version": version,
        "is_docker": is_docker,
        "os": os,
        "arch": arch
    })
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

        let total_users = state.budget_ledger.snapshot().len();

        let model_distribution = {
            let counts = state.model_counts.read().unwrap();
            counts.clone()
        };

        info!(
            "Sending 24hr cycle MAPD telemetry check-in to {}...",
            state.telemetry.endpoint
        );

        let payload = daily_telemetry_payload(
            &client_hash,
            current_version,
            total_requests,
            total_tokens,
            total_users,
            model_distribution,
        );

        let _ = client
            .post(&state.telemetry.endpoint)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
    }
}

fn daily_telemetry_payload(
    client_hash: &str,
    version: &str,
    total_requests: usize,
    total_tokens: usize,
    total_users: usize,
    model_distribution: HashMap<String, usize>,
) -> serde_json::Value {
    serde_json::json!({
        "type": "daily_mapd",
        "client_hash": client_hash,
        "version": version,
        "total_requests": total_requests,
        "total_tokens": total_tokens,
        "total_users": total_users,
        "model_distribution": model_distribution
    })
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

fn parse_bool(raw: Option<&str>, default: bool) -> bool {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

fn parse_positive_size(raw: Option<&str>, default: usize) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_optional_positive_size(
    raw: Option<&str>,
    variable: &str,
) -> Result<Option<usize>, String> {
    match raw {
        None => Ok(None),
        Some(value) => value
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|parsed| *parsed > 0)
            .map(Some)
            .ok_or_else(|| format!("{variable} must be a positive integer")),
    }
}

fn parse_budget_limit(raw: Option<&str>, default: f64, variable: &str) -> Result<f64, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let value = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{variable} must be a finite non-negative USD amount"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "{variable} must be a finite non-negative USD amount"
        ));
    }
    Ok(value)
}

fn validate_http_url(raw: &str, variable: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw)
        .map_err(|error| format!("{variable} must be an absolute HTTP(S) URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{variable} must be an absolute HTTP(S) URL"));
    }
    Ok(())
}

fn parse_strict_bool(raw: Option<&str>, variable: &str) -> Result<bool, String> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None | Some("0" | "false" | "no" | "off") => Ok(false),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some(_) => Err(format!(
            "{variable} must be one of true/false, 1/0, yes/no, or on/off"
        )),
    }
}

fn is_loopback_bind(bind: &str) -> bool {
    let bind = bind.trim();
    if bind.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(address) = bind.parse::<std::net::SocketAddr>() {
        return address.ip().is_loopback();
    }
    if let Ok(ip) = bind.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    if let Some((host, port)) = bind.rsplit_once(':')
        && port.parse::<u16>().is_ok()
    {
        return host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
    }
    false
}

fn validate_deployment_safety(
    bind: &str,
    acknowledge_process_local_ledger: bool,
    proxy_token_configured: bool,
    allow_unauthenticated_public_proxy: bool,
) -> Result<(), String> {
    if is_loopback_bind(bind) {
        return Ok(());
    }
    if !acknowledge_process_local_ledger {
        return Err(
            "Non-loopback startup requires KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER=true. \
             Spend resets on restart, multiple instances multiply the effective budget, and one \
             logical project budget must use one Kilovolt process."
                .to_string(),
        );
    }
    if !proxy_token_configured && !allow_unauthenticated_public_proxy {
        return Err(
            "Non-loopback startup requires KILOVOLT_PROXY_TOKEN or the explicit unsafe override \
             KILOVOLT_ALLOW_UNAUTHENTICATED_PUBLIC_PROXY=true."
                .to_string(),
        );
    }
    Ok(())
}

fn bind_address(bind: &str, port: u16) -> String {
    let bind = bind.trim();
    if bind.parse::<std::net::SocketAddr>().is_ok() {
        bind.to_string()
    } else if let Ok(ip) = bind.parse::<std::net::IpAddr>() {
        std::net::SocketAddr::new(ip, port).to_string()
    } else if bind
        .rsplit_once(':')
        .is_some_and(|(_, candidate_port)| candidate_port.parse::<u16>().is_ok())
    {
        bind.to_string()
    } else {
        format!("{bind}:{port}")
    }
}

fn fatal_configuration(message: &str) -> ! {
    error!("{message}");
    eprintln!("Kilovolt configuration error: {message}");
    std::process::exit(1);
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
    let bind = std::env::var("BIND_ADDR")
        .ok()
        .or_else(|| std::env::var("HOST").ok())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let addr = bind_address(&bind, port);

    let default_budget_raw = std::env::var("KILOVOLT_DEFAULT_BUDGET").ok();
    let default_budget = parse_budget_limit(
        default_budget_raw.as_deref(),
        1.00,
        "KILOVOLT_DEFAULT_BUDGET",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));

    let project_budget_raw = std::env::var("KILOVOLT_PROJECT_BUDGET").ok();
    let project_budget = parse_budget_limit(
        project_budget_raw.as_deref(),
        default_budget,
        "KILOVOLT_PROJECT_BUDGET",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));

    let per_step_tokens = std::env::var("KILOVOLT_PER_STEP_TOKENS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let per_pipeline_tokens = std::env::var("KILOVOLT_PER_PIPELINE_TOKENS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let per_day_tokens = std::env::var("KILOVOLT_PER_DAY_TOKENS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let request_body_limit_raw = std::env::var("KILOVOLT_MAX_REQUEST_BODY_BYTES").ok();
    let max_request_body_bytes = parse_positive_size(
        request_body_limit_raw.as_deref(),
        DEFAULT_MAX_REQUEST_BODY_BYTES,
    );
    if request_body_limit_raw.is_some()
        && request_body_limit_raw
            .as_deref()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .is_none()
    {
        warn!(
            default = DEFAULT_MAX_REQUEST_BODY_BYTES,
            "Invalid KILOVOLT_MAX_REQUEST_BODY_BYTES; using the safe default"
        );
    }

    let upstream_body_limit_raw = std::env::var("KILOVOLT_MAX_UPSTREAM_BODY_BYTES").ok();
    let max_upstream_body_bytes = parse_positive_size(
        upstream_body_limit_raw.as_deref(),
        DEFAULT_MAX_UPSTREAM_BODY_BYTES,
    );
    if upstream_body_limit_raw.is_some()
        && upstream_body_limit_raw
            .as_deref()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .is_none()
    {
        warn!(
            default = DEFAULT_MAX_UPSTREAM_BODY_BYTES,
            "Invalid KILOVOLT_MAX_UPSTREAM_BODY_BYTES; using the safe default"
        );
    }

    let sse_frame_limit_raw = std::env::var("KILOVOLT_MAX_SSE_FRAME_BYTES").ok();
    let max_sse_frame_bytes =
        parse_positive_size(sse_frame_limit_raw.as_deref(), DEFAULT_MAX_SSE_FRAME_BYTES);
    if sse_frame_limit_raw.is_some()
        && sse_frame_limit_raw
            .as_deref()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .is_none()
    {
        warn!(
            default = DEFAULT_MAX_SSE_FRAME_BYTES,
            "Invalid KILOVOLT_MAX_SSE_FRAME_BYTES; using the safe default"
        );
    }

    let non_stream_default_max_output_tokens = parse_optional_positive_size(
        std::env::var("KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS")
            .ok()
            .as_deref(),
        "KILOVOLT_NON_STREAM_DEFAULT_MAX_OUTPUT_TOKENS",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));

    let proxy_token = std::env::var("KILOVOLT_PROXY_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(Arc::<str>::from);
    let acknowledge_process_local_ledger = parse_strict_bool(
        std::env::var("KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER")
            .ok()
            .as_deref(),
        "KILOVOLT_ACKNOWLEDGE_PROCESS_LOCAL_LEDGER",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));
    let allow_unauthenticated_public_proxy = parse_strict_bool(
        std::env::var("KILOVOLT_ALLOW_UNAUTHENTICATED_PUBLIC_PROXY")
            .ok()
            .as_deref(),
        "KILOVOLT_ALLOW_UNAUTHENTICATED_PUBLIC_PROXY",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));
    let mock_upstream_enabled = parse_strict_bool(
        std::env::var("KILOVOLT_ENABLE_MOCK_UPSTREAM")
            .ok()
            .as_deref(),
        "KILOVOLT_ENABLE_MOCK_UPSTREAM",
    )
    .unwrap_or_else(|message| fatal_configuration(&message));
    if mock_upstream_enabled {
        warn!("Embedded mock upstream enabled for local evaluation; do not use it in production");
    }
    validate_deployment_safety(
        &bind,
        acknowledge_process_local_ledger,
        proxy_token.is_some(),
        allow_unauthenticated_public_proxy,
    )
    .unwrap_or_else(|message| fatal_configuration(&message));

    let pricing_file = std::env::var("KILOVOLT_PRICING_FILE").ok();
    let pricing_registry = PricingRegistry::load(pricing_file.as_deref().map(std::path::Path::new))
        .unwrap_or_else(|error| fatal_configuration(&format!("KILOVOLT_PRICING_FILE: {error}")));

    let dashboard_token = std::env::var("KILOVOLT_DASHBOARD_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(Arc::<str>::from);
    if dashboard_token.is_none() {
        warn!("Customer dashboard disabled: set KILOVOLT_DASHBOARD_TOKEN and restart to enable it");
    }

    let telemetry_enabled = parse_bool(
        std::env::var("KILOVOLT_TELEMETRY_ENABLED").ok().as_deref(),
        false,
    );
    let telemetry = TelemetryConfig {
        enabled: telemetry_enabled,
        endpoint: std::env::var("KILOVOLT_TELEMETRY_URL")
            .unwrap_or_else(|_| DEFAULT_TELEMETRY_URL.to_string()),
    };
    let openai_upstream_url = std::env::var("KILOVOLT_OPENAI_UPSTREAM_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".to_string());
    validate_http_url(&openai_upstream_url, "KILOVOLT_OPENAI_UPSTREAM_URL")
        .unwrap_or_else(|message| fatal_configuration(&message));
    let upstream_header_timeout_seconds = parse_positive_size(
        std::env::var("KILOVOLT_UPSTREAM_HEADER_TIMEOUT_SECONDS")
            .ok()
            .as_deref(),
        30,
    ) as u64;

    info!(
        port = %port,
        project_budget = %project_budget,
        default_budget = %default_budget,
        max_request_body_bytes = %max_request_body_bytes,
        max_upstream_body_bytes = %max_upstream_body_bytes,
        max_sse_frame_bytes = %max_sse_frame_bytes,
        non_stream_default_max_output_tokens = ?non_stream_default_max_output_tokens,
        pricing_file = ?pricing_file,
        operator_pricing_entries = %pricing_registry.operator_entry_count(),
        built_in_pricing_entries = %pricing_registry.built_in_entry_count(),
        built_in_pricing_verified = false,
        proxy_authentication_enabled = %proxy_token.is_some(),
        mock_upstream_enabled = %mock_upstream_enabled,
        process_local_ledger_acknowledged = %acknowledge_process_local_ledger,
        allow_unauthenticated_public_proxy = %allow_unauthenticated_public_proxy,
        bind_address = %addr,
        dashboard_enabled = %dashboard_token.is_some(),
        company_telemetry_enabled = %telemetry.enabled,
        openai_upstream_url = %openai_upstream_url,
        upstream_header_timeout_seconds = %upstream_header_timeout_seconds,
        per_step_tokens = ?per_step_tokens,
        per_pipeline_tokens = ?per_pipeline_tokens,
        per_day_tokens = ?per_day_tokens,
        "Configuration loaded successfully"
    );

    // Create the reqwest Client with a connection pool
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(100)
        .build()
        .expect("Failed to build reqwest client");

    // Initialize the atomic financial budget ledger.
    let budget_ledger = Arc::new(BudgetLedger::new());

    // Retrieve or create client identity hash
    let client_hash = if telemetry.enabled {
        get_or_create_client_hash()
    } else {
        "telemetry-disabled".to_string()
    };
    let model_counts = Arc::new(RwLock::new(HashMap::new()));

    let state = AppState {
        client: client.clone(),
        budget_ledger,
        project_budget,
        default_budget,
        port,
        openai_upstream_url,
        upstream_header_timeout: std::time::Duration::from_secs(upstream_header_timeout_seconds),
        max_request_body_bytes,
        max_upstream_body_bytes,
        max_sse_frame_bytes,
        non_stream_default_max_output_tokens,
        pricing_registry: Arc::new(pricing_registry),
        proxy_token,
        mock_upstream_enabled,
        dashboard_token,
        telemetry: telemetry.clone(),
        per_step_tokens,
        per_pipeline_tokens,
        per_day_tokens,
        tokens_used_today: Arc::new(AtomicUsize::new(0)),
        day_start: Arc::new(RwLock::new(chrono::Local::now().date_naive())),
        pipeline_tracker: Arc::new(RwLock::new(HashMap::new())),
        start_time: Instant::now(),
        total_requests: Arc::new(AtomicUsize::new(0)),
        total_latency_ms: Arc::new(AtomicU64::new(0)),
        total_tokens_consumed: Arc::new(AtomicUsize::new(0)),
        recent_requests: Arc::new(Mutex::new(VecDeque::new())),
        client_hash: client_hash.clone(),
        model_counts,
    };

    if telemetry.enabled {
        let startup_client = client.clone();
        let startup_hash = client_hash.clone();
        let startup_endpoint = telemetry.endpoint.clone();
        tokio::spawn(async move {
            send_startup_telemetry(startup_client, startup_hash, startup_endpoint).await;
        });

        let daily_state = state.clone();
        tokio::spawn(async move {
            run_daily_telemetry_loop(daily_state).await;
        });
    }

    // Build the Axum Router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/dashboard", get(get_dashboard))
        .route("/api/stats", get(get_stats))
        .route("/v1/chat/completions", post(chat_completions_proxy))
        .route("/mock/v1/chat/completions", post(mock_chat_completions))
        .with_state(state);

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

#[cfg(test)]
mod tests {
    use super::{
        bind_address, daily_telemetry_payload, is_loopback_bind, parse_bool, parse_budget_limit,
        parse_optional_positive_size, parse_positive_size, parse_strict_bool,
        startup_telemetry_payload, validate_deployment_safety, validate_http_url,
    };
    use std::collections::HashMap;

    #[test]
    fn safe_size_configuration_uses_default_when_missing_or_invalid() {
        assert_eq!(parse_positive_size(None, 1024), 1024);
        assert_eq!(parse_positive_size(Some("invalid"), 1024), 1024);
        assert_eq!(parse_positive_size(Some("0"), 1024), 1024);
        assert_eq!(parse_positive_size(Some("2048"), 1024), 2048);
    }

    #[test]
    fn telemetry_is_disabled_for_missing_or_invalid_configuration() {
        assert!(!parse_bool(None, false));
        assert!(!parse_bool(Some("invalid"), false));
        assert!(parse_bool(Some("true"), false));
        assert!(!parse_bool(Some("off"), true));
    }

    #[test]
    fn non_stream_default_and_security_booleans_fail_closed() {
        assert_eq!(
            parse_optional_positive_size(None, "TEST").expect("missing is optional"),
            None
        );
        assert_eq!(
            parse_optional_positive_size(Some("1000"), "TEST")
                .expect("positive integer should parse"),
            Some(1000)
        );
        assert!(parse_optional_positive_size(Some("0"), "TEST").is_err());
        assert!(parse_strict_bool(Some("invalid"), "TEST").is_err());
        assert!(parse_strict_bool(Some("true"), "TEST").unwrap());
    }

    #[test]
    fn financial_limits_must_be_finite_and_non_negative() {
        assert_eq!(parse_budget_limit(None, 1.0, "TEST").unwrap(), 1.0);
        assert_eq!(
            parse_budget_limit(Some(" 0.25 "), 1.0, "TEST").unwrap(),
            0.25
        );
        for invalid in ["", "not-a-number", "-0.01", "NaN", "inf", "-inf"] {
            assert!(
                parse_budget_limit(Some(invalid), 1.0, "TEST").is_err(),
                "{invalid} should fail"
            );
        }
    }

    #[test]
    fn upstream_url_must_be_absolute_http_or_https() {
        for valid in [
            "https://api.openai.com/v1/chat/completions",
            "http://127.0.0.1:11434/v1/chat/completions",
        ] {
            assert!(validate_http_url(valid, "TEST").is_ok(), "{valid}");
        }
        for invalid in [
            "api.openai.com/v1/chat/completions",
            "/v1/chat/completions",
            "ftp://example.com/model",
            "http://",
        ] {
            assert!(validate_http_url(invalid, "TEST").is_err(), "{invalid}");
        }
    }

    #[test]
    fn loopback_and_non_loopback_safety_rules_are_explicit() {
        for bind in [
            "127.0.0.1",
            "127.0.0.1:8080",
            "::1",
            "[::1]:8080",
            "localhost",
        ] {
            assert!(is_loopback_bind(bind), "{bind} should be loopback");
            assert!(validate_deployment_safety(bind, false, false, false).is_ok());
        }
        assert!(!is_loopback_bind("0.0.0.0"));
        assert!(validate_deployment_safety("0.0.0.0", false, true, false).is_err());
        assert!(validate_deployment_safety("0.0.0.0", true, false, false).is_err());
        assert!(validate_deployment_safety("0.0.0.0", true, true, false).is_ok());
        assert!(validate_deployment_safety("0.0.0.0", true, false, true).is_ok());
        assert_eq!(bind_address("127.0.0.1", 8080), "127.0.0.1:8080");
        assert_eq!(bind_address("::1", 8080), "[::1]:8080");
        assert_eq!(bind_address("0.0.0.0:9000", 8080), "0.0.0.0:9000");
    }

    #[test]
    fn startup_and_daily_telemetry_payload_fields_are_explicit() {
        assert_eq!(
            startup_telemetry_payload("hash", "1.2.3", true, "linux", "amd64"),
            serde_json::json!({
                "type": "startup",
                "client_hash": "hash",
                "version": "1.2.3",
                "is_docker": true,
                "os": "linux",
                "arch": "amd64"
            })
        );
        assert_eq!(
            daily_telemetry_payload(
                "hash",
                "1.2.3",
                10,
                20,
                2,
                HashMap::from([("gpt-test".to_string(), 10)])
            ),
            serde_json::json!({
                "type": "daily_mapd",
                "client_hash": "hash",
                "version": "1.2.3",
                "total_requests": 10,
                "total_tokens": 20,
                "total_users": 2,
                "model_distribution": {"gpt-test": 10}
            })
        );
    }
}
