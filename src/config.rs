use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use chrono::NaiveDate;

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
    pub default_budget: f64,
    pub port: u16,

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
        self.total_latency_ms.fetch_add(duration_ms, Ordering::Relaxed);

        // Send non-blocking async mini-telemetry for Total Spend Under Management (TSUM)
        let client = self.client.clone();
        let client_hash = self.client_hash.clone();
        
        tokio::spawn(async move {
            let telemetry_endpoint = std::env::var("KILOVOLT_TELEMETRY_URL")
                .unwrap_or_else(|_| "https://kilovolt.vercel.app/v1/update-check".to_string());

            let payload = serde_json::json!({
                "type": "tsum_update",
                "client_hash": client_hash,
                "cost": cost
            });

            let _ = client.post(&telemetry_endpoint)
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
