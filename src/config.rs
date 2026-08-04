use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use std::time::Instant;

use chrono::NaiveDate;

use crate::ledger::BudgetLedger;
use crate::pricing::PricingRegistry;

pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_UPSTREAM_BODY_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_SSE_FRAME_BYTES: usize = 256 * 1024;
pub const DEFAULT_TELEMETRY_URL: &str = "https://kilovolt.vercel.app/v1/update-check";

#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub endpoint: String,
}

/// Temporary browser-configured credentials and limits for the local evaluation
/// flow. This type intentionally implements neither `Debug` nor serialization so
/// provider and gateway credentials cannot be logged accidentally.
#[derive(Clone)]
pub struct EvaluationSetup {
    provider_api_key: Arc<str>,
    gateway_key: Arc<str>,
    project_budget: f64,
    default_budget: f64,
}

impl EvaluationSetup {
    pub fn provider_api_key(&self) -> &str {
        &self.provider_api_key
    }

    pub fn gateway_key(&self) -> &str {
        &self.gateway_key
    }

    pub fn budgets(&self) -> (f64, f64) {
        (self.project_budget, self.default_budget)
    }
}

#[derive(Clone)]
pub struct EvaluationTestResult {
    pub model: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub output_text: String,
    pub spend_usd: f64,
    pub latency_ms: u64,
    pub project_calculated_spend_usd: f64,
}

pub struct EvaluationSetupState {
    configured: RwLock<Option<EvaluationSetup>>,
    test_succeeded: AtomicBool,
    last_test_result: RwLock<Option<EvaluationTestResult>>,
}

impl EvaluationSetupState {
    pub fn new() -> Self {
        Self {
            configured: RwLock::new(None),
            test_succeeded: AtomicBool::new(false),
            last_test_result: RwLock::new(None),
        }
    }

    pub fn snapshot(&self) -> Option<EvaluationSetup> {
        self.configured.read().unwrap().clone()
    }

    pub fn complete(
        &self,
        provider_api_key: Arc<str>,
        gateway_key: Arc<str>,
        project_budget: f64,
        default_budget: f64,
    ) -> bool {
        let mut configured = self.configured.write().unwrap();
        if configured.is_some() {
            return false;
        }
        *configured = Some(EvaluationSetup {
            provider_api_key,
            gateway_key,
            project_budget,
            default_budget,
        });
        true
    }

    pub fn update_budgets(&self, project_budget: f64, default_budget: f64) -> bool {
        if !project_budget.is_finite()
            || project_budget < 0.0
            || !default_budget.is_finite()
            || default_budget < 0.0
        {
            return false;
        }

        let mut configured = self.configured.write().unwrap();
        let Some(setup) = configured.as_mut() else {
            return false;
        };
        setup.project_budget = project_budget;
        setup.default_budget = default_budget;
        true
    }

    pub fn mark_test_succeeded(&self) {
        self.test_succeeded.store(true, Ordering::Release);
    }

    pub fn save_test_result(&self, result: EvaluationTestResult) {
        *self.last_test_result.write().unwrap() = Some(result);
        self.mark_test_succeeded();
    }

    pub fn test_result(&self) -> Option<EvaluationTestResult> {
        self.last_test_result.read().unwrap().clone()
    }

    pub fn test_succeeded(&self) -> bool {
        self.test_succeeded.load(Ordering::Acquire)
    }
}

impl Default for EvaluationSetupState {
    fn default() -> Self {
        Self::new()
    }
}

/// Compares secret bytes without data-dependent early exit.
pub fn secrets_match(expected: &str, actual: &str) -> bool {
    let expected_bytes = expected.as_bytes();
    let actual_bytes = actual.as_bytes();
    let maximum_length = expected_bytes.len().max(actual_bytes.len());
    let mut difference = expected_bytes.len() ^ actual_bytes.len();
    for index in 0..maximum_length {
        let left = expected_bytes.get(index).copied().unwrap_or(0);
        let right = actual_bytes.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
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
    pub non_stream_default_max_output_tokens: Option<usize>,
    pub pricing_registry: Arc<PricingRegistry>,
    pub proxy_token: Option<Arc<str>>,
    pub mock_upstream_enabled: bool,
    pub dashboard_token: Option<Arc<str>>,
    pub evaluation_setup: Option<Arc<EvaluationSetupState>>,
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
    pub fn evaluation_mode(&self) -> bool {
        self.evaluation_setup.is_some()
    }

    pub fn evaluation_setup_snapshot(&self) -> Option<EvaluationSetup> {
        self.evaluation_setup
            .as_ref()
            .and_then(|setup| setup.snapshot())
    }

    pub fn evaluation_setup_complete(&self) -> bool {
        self.evaluation_setup_snapshot().is_some()
    }

    pub fn complete_evaluation_setup(
        &self,
        provider_api_key: Arc<str>,
        gateway_key: Arc<str>,
        project_budget: f64,
        default_budget: f64,
    ) -> bool {
        self.evaluation_setup.as_ref().is_some_and(|setup| {
            setup.complete(
                provider_api_key,
                gateway_key,
                project_budget,
                default_budget,
            )
        })
    }

    pub fn effective_budgets(&self) -> (f64, f64) {
        self.evaluation_setup_snapshot()
            .map_or((self.project_budget, self.default_budget), |setup| {
                setup.budgets()
            })
    }

    pub fn update_evaluation_budgets(&self, project_budget: f64, default_budget: f64) -> bool {
        self.evaluation_setup
            .as_ref()
            .is_some_and(|setup| setup.update_budgets(project_budget, default_budget))
    }

    pub fn evaluation_test_succeeded(&self) -> bool {
        self.evaluation_setup
            .as_ref()
            .is_some_and(|setup| setup.test_succeeded())
    }

    pub fn save_evaluation_test_result(&self, result: EvaluationTestResult) {
        if let Some(setup) = &self.evaluation_setup {
            setup.save_test_result(result);
        }
    }

    pub fn evaluation_test_result(&self) -> Option<EvaluationTestResult> {
        self.evaluation_setup
            .as_ref()
            .and_then(|setup| setup.test_result())
    }

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
        non_stream_default_max_output_tokens: Some(1_024),
        pricing_registry: Arc::new(PricingRegistry::built_in()),
        proxy_token: None,
        mock_upstream_enabled: true,
        dashboard_token: Some(Arc::from("test-dashboard-token")),
        evaluation_setup: None,
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
pub(crate) fn test_evaluation_state(port: u16) -> AppState {
    let mut state = test_state_with_budgets(port, 10.0, 1.0);
    state.mock_upstream_enabled = false;
    state.dashboard_token = None;
    state.evaluation_setup = Some(Arc::new(EvaluationSetupState::new()));
    state
}

#[cfg(test)]
mod tests {
    use super::{EvaluationTestResult, secrets_match, test_evaluation_state, test_state};
    use axum::{Json, Router, routing::post};
    use serde_json::Value;
    use std::time::Duration;

    #[test]
    fn secret_comparison_handles_matches_mismatches_and_lengths() {
        assert!(secrets_match("proxy-secret", "proxy-secret"));
        assert!(!secrets_match("proxy-secret", "proxy-secreu"));
        assert!(!secrets_match("proxy-secret", "proxy-secret-longer"));
        assert!(!secrets_match("proxy-secret", ""));
    }

    #[test]
    fn evaluation_setup_is_one_time_and_supplies_effective_budgets() {
        let state = test_evaluation_state(0);
        assert!(state.evaluation_mode());
        assert!(!state.evaluation_setup_complete());
        assert!(!state.evaluation_test_succeeded());
        assert_eq!(state.effective_budgets(), (10.0, 1.0));

        assert!(state.complete_evaluation_setup(
            "provider-secret".into(),
            "gateway-secret".into(),
            4.0,
            0.5,
        ));
        assert!(!state.complete_evaluation_setup(
            "replacement-provider".into(),
            "replacement-gateway".into(),
            8.0,
            2.0,
        ));

        let configured = state
            .evaluation_setup_snapshot()
            .expect("evaluation setup should be configured");
        assert_eq!(configured.provider_api_key(), "provider-secret");
        assert_eq!(configured.gateway_key(), "gateway-secret");
        assert_eq!(state.effective_budgets(), (4.0, 0.5));
        assert!(state.update_evaluation_budgets(6.0, 0.75));
        assert_eq!(state.effective_budgets(), (6.0, 0.75));
        assert!(!state.update_evaluation_budgets(f64::NAN, 1.0));
        assert!(!state.update_evaluation_budgets(1.0, -1.0));
        assert_eq!(state.effective_budgets(), (6.0, 0.75));
        let updated = state
            .evaluation_setup_snapshot()
            .expect("evaluation setup should remain configured");
        assert_eq!(updated.provider_api_key(), "provider-secret");
        assert_eq!(updated.gateway_key(), "gateway-secret");
        state.save_evaluation_test_result(EvaluationTestResult {
            model: "gpt-4o-mini".to_string(),
            input_tokens: 8,
            output_tokens: 4,
            output_text: "Kilovolt is working.".to_string(),
            spend_usd: 0.000_006,
            latency_ms: 250,
            project_calculated_spend_usd: 0.000_006,
        });
        assert!(state.evaluation_test_succeeded());
        let test_result = state
            .evaluation_test_result()
            .expect("successful evaluation result should be retained in memory");
        assert_eq!(test_result.input_tokens, 8);
        assert_eq!(test_result.output_tokens, 4);
        assert_eq!(test_result.output_text, "Kilovolt is working.");
    }

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
