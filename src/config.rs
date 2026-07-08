use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
    pub default_budget: f64,
    pub port: u16,

    // Telemetry and stats tracking
    pub start_time: Instant,
    pub total_requests: Arc<AtomicUsize>,
    pub total_latency_ms: Arc<AtomicU64>,
    pub total_tokens_consumed: Arc<AtomicUsize>,
    pub recent_requests: Arc<Mutex<VecDeque<RecentRequest>>>,
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

        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(duration_ms, Ordering::Relaxed);
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
