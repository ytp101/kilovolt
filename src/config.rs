use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use std::time::Instant;

use chrono::NaiveDate;

use crate::ledger::BudgetLedger;

pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_UPSTREAM_BODY_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_SSE_FRAME_BYTES: usize = 256 * 1024;
pub const DEFAULT_TELEMETRY_URL: &str = "https://kilovolt.vercel.app/v1/update-check";

#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub endpoint: String,
}

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub budget_ledger: Arc<BudgetLedger>,
    pub project_budget: f64,
    pub default_budget: f64,
    pub port: u16,
    pub openai_upstream_url: String,
    pub upstream_header_timeout: Duration,
    pub max_request_body_bytes: usize,
    pub max_upstream_body_bytes: usize,
    pub max_sse_frame_bytes: usize,
    pub dashboard_token: Option<Arc<str>>,
    pub telemetry: TelemetryConfig,

    // Token budget configuration
    pub per_step_tokens: Option<usize>,
    pub per_pipeline_tokens: Option<usize>,
    pub per_day_tokens: Option<usize>,

    // Token budget trackers
    pub tokens_used_today: Arc<AtomicUsize>,
    pub day_start: Arc<RwLock<NaiveDate>>,
    pub pipeline_tracker: Arc<RwLock<HashMap<String, usize>>>,

    // Telemetry and stats tracking
    pub start_time: Instant,
    pub total_requests: Arc<AtomicUsize>,
    pub total_latency_ms: Arc<AtomicU64>,
    pub total_tokens_consumed: Arc<AtomicUsize>,
    pub recent_requests: Arc<Mutex<VecDeque<RecentRequest>>>,

    // New telemetry metrics
    pub client_hash: String,
    pub model_counts: Arc<RwLock<HashMap<String, usize>>>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn record_request(
        &self,
        request_id: &str,
        user_id: &str,
        model: &str,
        status: u16,
        duration_ms: u64,
        tokens: usize,
        cost: f64,
    ) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let record = RecentRequest {
            request_id: request_id.to_string(),
            timestamp,
            user_id: user_id.to_string(),
            model: model.to_string(),
            status,
            duration_ms,
            tokens,
            cost,
        };

        {
            let mut list = self.recent_requests.lock().unwrap();
            list.push_front(record);
            if list.len() > 5 {
                list.pop_back();
            }
        }

        {
            let mut counts = self.model_counts.write().unwrap();
            let entry = counts.entry(model.to_string()).or_insert(0);
            *entry += 1;
        }

        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(duration_ms, Ordering::Relaxed);

        if !self.telemetry.enabled {
            return;
        }

        // Send non-blocking company telemetry only after explicit opt-in.
        let client = self.client.clone();
        let client_hash = self.client_hash.clone();
        let telemetry_endpoint = self.telemetry.endpoint.clone();

        tokio::spawn(async move {
            let payload = serde_json::json!({
                "type": "tsum_update",
                "client_hash": client_hash,
                "cost": cost
            });

            let _ = client
                .post(&telemetry_endpoint)
                .json(&payload)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await;
        });
    }
}

// Struct for dashboard and stats API payloads
#[derive(serde::Serialize, Clone)]
pub struct RecentRequest {
    pub request_id: String,
    pub timestamp: String,
    pub user_id: String,
    pub model: String,
    pub status: u16,
    pub duration_ms: u64,
    pub tokens: usize,
    pub cost: f64,
}

#[cfg(test)]
pub(crate) fn test_state(port: u16, default_budget: f64) -> AppState {
    test_state_with_budgets(port, default_budget, default_budget)
}

#[cfg(test)]
pub(crate) fn test_state_with_budgets(
    port: u16,
    project_budget: f64,
    default_budget: f64,
) -> AppState {
    AppState {
        client: reqwest::Client::builder()
            .build()
            .expect("test HTTP client should build"),
        budget_ledger: Arc::new(BudgetLedger::new()),
        project_budget,
        default_budget,
        port,
        openai_upstream_url: "https://api.openai.com/v1/chat/completions".to_string(),
        upstream_header_timeout: Duration::from_secs(1),
        max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        max_upstream_body_bytes: DEFAULT_MAX_UPSTREAM_BODY_BYTES,
        max_sse_frame_bytes: DEFAULT_MAX_SSE_FRAME_BYTES,
        dashboard_token: Some(Arc::from("test-dashboard-token")),
        telemetry: TelemetryConfig {
            enabled: false,
            endpoint: DEFAULT_TELEMETRY_URL.to_string(),
        },
        per_step_tokens: None,
        per_pipeline_tokens: None,
        per_day_tokens: None,
        tokens_used_today: Arc::new(AtomicUsize::new(0)),
        day_start: Arc::new(RwLock::new(chrono::Local::now().date_naive())),
        pipeline_tracker: Arc::new(RwLock::new(HashMap::new())),
        start_time: Instant::now(),
        total_requests: Arc::new(AtomicUsize::new(0)),
        total_latency_ms: Arc::new(AtomicU64::new(0)),
        total_tokens_consumed: Arc::new(AtomicUsize::new(0)),
        recent_requests: Arc::new(Mutex::new(VecDeque::new())),
        client_hash: "test-client".to_string(),
        model_counts: Arc::new(RwLock::new(HashMap::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::test_state;
    use axum::{Json, Router, routing::post};
    use serde_json::Value;
    use std::time::Duration;

    async fn telemetry_receiver(
        sender: tokio::sync::mpsc::UnboundedSender<Value>,
        Json(payload): Json<Value>,
    ) {
        let _ = sender.send(payload);
    }

    async fn spawn_receiver() -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new().route(
            "/telemetry",
            post(move |payload| telemetry_receiver(sender.clone(), payload)),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("telemetry receiver should bind");
        let address = listener
            .local_addr()
            .expect("telemetry receiver should have an address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("telemetry receiver should serve");
        });
        (format!("http://{address}/telemetry"), receiver, server)
    }

    #[tokio::test]
    async fn disabled_telemetry_sends_no_request() {
        let (endpoint, mut receiver, server) = spawn_receiver().await;
        let mut state = test_state(0, 1.0);
        state.telemetry.enabled = false;
        state.telemetry.endpoint = endpoint;

        state.record_request("request", "user", "model", 200, 1, 2, 0.25);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                .await
                .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    async fn enabled_request_telemetry_sends_only_documented_fields() {
        let (endpoint, mut receiver, server) = spawn_receiver().await;
        let mut state = test_state(0, 1.0);
        state.telemetry.enabled = true;
        state.telemetry.endpoint = endpoint;

        state.record_request("request", "user", "model", 200, 1, 2, 0.25);
        let payload = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("telemetry receiver should be contacted")
            .expect("telemetry payload should be sent");
        assert_eq!(
            payload,
            serde_json::json!({
                "type": "tsum_update",
                "client_hash": "test-client",
                "cost": 0.25
            })
        );
        server.abort();
    }
}
