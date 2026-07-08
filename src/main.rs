use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::Instant;
use tiktoken_rs::{bpe_for_model, num_tokens_from_messages, ChatCompletionRequestMessage};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Define AppState to share configurations and tracking map across request threads
#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
    default_budget: f64,
    port: u16,

    // Telemetry and stats tracking
    start_time: Instant,
    total_requests: Arc<AtomicUsize>,
    total_latency_ms: Arc<AtomicU64>,
    total_tokens_consumed: Arc<AtomicUsize>,
    recent_requests: Arc<Mutex<VecDeque<RecentRequest>>>,
}

impl AppState {
    fn record_request(&self, user_id: &str, model: &str, status: u16, duration_ms: u64) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        let record = RecentRequest {
            timestamp,
            user_id: user_id.to_string(),
            model: model.to_string(),
            status,
            duration_ms,
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

// Structs for incoming request body parsing
#[derive(serde::Deserialize, Clone)]
#[allow(dead_code)]
struct IncomingRequest {
    model: String,
    messages: Vec<IncomingMessage>,
    #[serde(default)]
    stream: bool,
}

#[derive(serde::Deserialize, Clone)]
struct IncomingMessage {
    role: String,
    content: Option<serde_json::Value>,
    name: Option<String>,
}

// Model-specific pricing configuration struct
#[derive(Clone, Copy)]
struct ModelPricing {
    input_cost_per_token: f64,
    output_cost_per_token: f64,
}

/// Dynamic pricing matrix loader based on OpenAI model definitions.
fn get_model_pricing(model: &str) -> ModelPricing {
    match model {
        m if m.starts_with("gpt-4o-mini") => ModelPricing {
            input_cost_per_token: 0.15 / 1_000_000.0,
            output_cost_per_token: 0.60 / 1_000_000.0,
        },
        m if m.starts_with("gpt-4o") => ModelPricing {
            input_cost_per_token: 5.00 / 1_000_000.0,
            output_cost_per_token: 15.00 / 1_000_000.0,
        },
        m if m.starts_with("gpt-4") => ModelPricing {
            input_cost_per_token: 30.00 / 1_000_000.0,
            output_cost_per_token: 60.00 / 1_000_000.0,
        },
        m if m.starts_with("gpt-3.5-turbo") => ModelPricing {
            input_cost_per_token: 0.50 / 1_000_000.0,
            output_cost_per_token: 1.50 / 1_000_000.0,
        },
        _ => ModelPricing {
            // Default fallback to gpt-4o pricing
            input_cost_per_token: 5.00 / 1_000_000.0,
            output_cost_per_token: 15.00 / 1_000_000.0,
        },
    }
}

// OpenAI-compatible error response structures
#[derive(serde::Serialize)]
struct OpenAIError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    param: Option<String>,
    code: Option<String>,
}

#[derive(serde::Serialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

/// Helper function to build structured OpenAI-compatible error responses.
fn make_error_response(
    status: StatusCode,
    message: &str,
    error_type: &str,
    code: Option<&str>,
) -> Response {
    let err = OpenAIErrorResponse {
        error: OpenAIError {
            message: message.to_string(),
            error_type: error_type.to_string(),
            param: None,
            code: code.map(String::from),
        },
    };
    (status, Json(err)).into_response()
}

/// Simple health check probe.
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// A mock upstream chat completion route that returns a standard chunked SSE event stream
/// with a artificial delay between events. Useful for verifying reverse-proxy streaming and termination.
async fn mock_chat_completions() -> Response {
    info!("Handling mock chat completions upstream request");
    let chunks = vec![
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1677825464,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1677825464,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1677825464,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" from\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-123\",\"object\":\"chat.completion.chunk\",\"created\":1677825464,\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" Kilovolt!\"},\"finish_reason\":null}]}\n\n",
        "data: [DONE]\n\n",
    ];

    let stream = stream::unfold((chunks, 0), |(chunks, index)| async move {
        if index >= chunks.len() {
            None
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let chunk = chunks[index];
            Some((Ok::<Bytes, std::io::Error>(Bytes::from(chunk)), (chunks, index + 1)))
        }
    });

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache");

    builder.body(Body::from_stream(stream)).unwrap()
}

/// Boxed stream type for reqwest chunk streaming.
type BoxedByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

/// Custom Stream wrapper designed to monitor chunk metrics, track spend aggregation,
/// and handle client-side disconnections without loading the stream into memory.
struct StreamMonitor<S> {
    inner: S,
    start_time: Instant,
    bytes_written: usize,
    chunks_written: usize,
    logged: bool,
    user_id: String,
    spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
    model: String,
    pricing: ModelPricing,
    prompt_cost: f64,
    bpe: Option<tiktoken_rs::CoreBPE>,
    accumulated_text: String,
    total_spend: f64,
    budget_limit: f64,
    output_tokens_count: usize,
    state: AppState,
}

impl<S> StreamMonitor<S> {
    fn new(
        inner: S,
        user_id: String,
        spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
        model: String,
        pricing: ModelPricing,
        prompt_cost: f64,
        total_spend: f64,
        budget_limit: f64,
        state: AppState,
    ) -> Self {
        let bpe = bpe_for_model(&model)
            .ok()
            .or_else(|| bpe_for_model("gpt-4o").ok())
            .cloned();

        Self {
            inner,
            start_time: Instant::now(),
            bytes_written: 0,
            chunks_written: 0,
            logged: false,
            user_id,
            spend_tracker,
            model,
            pricing,
            prompt_cost,
            bpe,
            accumulated_text: String::new(),
            total_spend,
            budget_limit,
            output_tokens_count: 0,
            state,
        }
    }

    /// Logs the final summary of the stream upon completion or cutoff.
    fn log_final_status(&mut self, is_cutoff: bool, outcome: &str) {
        if self.logged {
            return;
        }
        let duration = self.start_time.elapsed();
        let duration_ms = duration.as_millis() as u64;
        let total_output_cost =
            self.output_tokens_count as f64 * self.pricing.output_cost_per_token;

        info!(
            user_id = %self.user_id,
            model = %self.model,
            duration = ?duration,
            chunks = %self.chunks_written,
            bytes = %self.bytes_written,
            prompt_cost = %self.prompt_cost,
            output_tokens = %self.output_tokens_count,
            output_cost = %total_output_cost,
            final_total_spend = %self.total_spend,
            cutoff = %is_cutoff,
            "Stream closed: {}", outcome
        );

        // Record request status for dashboard and average latency tracking
        let status_code = if is_cutoff { 429 } else { 200 };
        self.state.record_request(&self.user_id, &self.model, status_code, duration_ms);

        self.logged = true;
    }
}

impl<S, E> Stream for StreamMonitor<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    type Item = Result<Bytes, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // 1. Guard check: Trip the circuit breaker if budget is already breached before polling
        if this.total_spend >= this.budget_limit {
            warn!(
                user_id = %this.user_id,
                total_spend = %this.total_spend,
                budget_limit = %this.budget_limit,
                "Bankruptcy Shield tripped PRE-POLL. Severing connection."
            );
            this.log_final_status(true, "tripped mid-stream");
            return Poll::Ready(None); // Terminate response stream early
        }

        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                this.bytes_written += bytes.len();
                this.chunks_written += 1;

                // 2. Parse chunk to trace dynamic SSE token generation
                let text_chunk = String::from_utf8_lossy(&bytes);
                this.accumulated_text.push_str(&text_chunk);

                while let Some(pos) = this.accumulated_text.find('\n') {
                    let line = this.accumulated_text[..pos].trim().to_string();
                    this.accumulated_text = this.accumulated_text[pos + 1..].to_string();

                    if line.starts_with("data:") {
                        let data_content = line["data:".len()..].trim();
                        if data_content == "[DONE]" {
                            continue;
                        }

                        // Parse the JSON data frame to extract generated text delta
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_content) {
                            if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                if let Some(choice) = choices.first() {
                                    if let Some(delta) = choice.get("delta") {
                                        if let Some(content) =
                                            delta.get("content").and_then(|c| c.as_str())
                                        {
                                            if let Some(bpe) = &this.bpe {
                                                let tokens = bpe.encode_with_special_tokens(content);
                                                let new_tokens = tokens.len();
                                                this.output_tokens_count += new_tokens;

                                                // Update total tokens consumed globally
                                                this.state.total_tokens_consumed.fetch_add(new_tokens, Ordering::Relaxed);

                                                // Update state tracker dynamically mid-stream
                                                let incremental_cost = new_tokens as f64
                                                    * this.pricing.output_cost_per_token;
                                                let new_spend = {
                                                    let mut map =
                                                        this.spend_tracker.write().unwrap();
                                                    let entry = map
                                                        .entry(this.user_id.clone())
                                                        .or_insert(0.0);
                                                    *entry += incremental_cost;
                                                    *entry
                                                };
                                                this.total_spend = new_spend;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // 3. Post-chunk evaluation: Sever socket if threshold is breached
                if this.total_spend >= this.budget_limit {
                    warn!(
                        user_id = %this.user_id,
                        total_spend = %this.total_spend,
                        budget_limit = %this.budget_limit,
                        "Bankruptcy Shield tripped MID-STREAM. Severing connection."
                    );
                    this.log_final_status(true, "tripped mid-stream");
                    return Poll::Ready(None); // Drop connection immediately
                }

                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                let duration = this.start_time.elapsed();
                error!(
                    "Stream failed after {:.2?} (chunks: {}, bytes: {}): {:?}",
                    duration,
                    this.chunks_written,
                    this.bytes_written,
                    err
                );
                this.log_final_status(false, "stream failed");
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                this.log_final_status(false, "completed successfully");
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// When the downstream client cancels the connection, Axum drops the stream,
// triggering this Drop implementation. This cleans up and logs the cancellation.
impl<S> Drop for StreamMonitor<S> {
    fn drop(&mut self) {
        if !self.logged {
            self.log_final_status(false, "aborted by client");
        }
    }
}

/// The core chat completions reverse proxy endpoint handler.
async fn chat_completions_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    info!("Ingesting POST /v1/chat/completions request");
    let start_time = Instant::now();

    // 1. Extract and validate Authorization header
    let auth_val = match headers.get(axum::http::header::AUTHORIZATION) {
        Some(val) => {
            let val_str = match val.to_str() {
                Ok(s) => s,
                Err(_) => {
                    warn!("Invalid UTF-8 in Authorization header");
                    state.record_request("anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64);
                    return make_error_response(
                        StatusCode::UNAUTHORIZED,
                        "Authorization header contains invalid character set",
                        "invalid_request_error",
                        Some("invalid_api_key"),
                    );
                }
            };
            if !val_str.starts_with("Bearer ") {
                warn!("Authorization header does not start with 'Bearer '");
                state.record_request("anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64);
                return make_error_response(
                    StatusCode::UNAUTHORIZED,
                    "Authorization header must start with 'Bearer '",
                    "invalid_request_error",
                    Some("invalid_api_key"),
                );
            }
            val.clone()
        }
        None => {
            warn!("Missing Authorization header");
            state.record_request("anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64);
            return make_error_response(
                StatusCode::UNAUTHORIZED,
                "Authorization header is missing",
                "invalid_request_error",
                Some("invalid_api_key"),
            );
        }
    };

    // 2. Extract and validate Content-Type header
    let content_type_val = match headers.get(axum::http::header::CONTENT_TYPE) {
        Some(val) => {
            let val_str = match val.to_str() {
                Ok(s) => s,
                Err(_) => {
                    warn!("Invalid UTF-8 in Content-Type header");
                    state.record_request("anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64);
                    return make_error_response(
                        StatusCode::BAD_REQUEST,
                        "Content-Type header is invalid",
                        "invalid_request_error",
                        None,
                    );
                }
            };
            if !val_str.starts_with("application/json") {
                warn!("Unsupported Content-Type: {}", val_str);
                state.record_request("anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64);
                return make_error_response(
                    StatusCode::BAD_REQUEST,
                    "Content-Type must be application/json",
                    "invalid_request_error",
                    None,
                );
            }
            val.clone()
        }
        None => {
            warn!("Missing Content-Type header");
            state.record_request("anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64);
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Content-Type header is missing",
                "invalid_request_error",
                None,
            );
        }
    };

    // 3. Extract Identity: X-User-ID header (defaults to "anonymous")
    let user_id = headers
        .get("x-user-id")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    // 4. Read request body bytes to calculate prompt token count
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to read request body bytes: {:?}", e);
            state.record_request(&user_id, "unknown", 400, start_time.elapsed().as_millis() as u64);
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Failed to read request body",
                "invalid_request_error",
                None,
            );
        }
    };

    // 5. Parse request body JSON
    let request: IncomingRequest = match serde_json::from_slice(&body_bytes) {
        Ok(req) => req,
        Err(e) => {
            warn!("Failed to deserialize request JSON: {:?}", e);
            state.record_request(&user_id, "unknown", 400, start_time.elapsed().as_millis() as u64);
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Invalid JSON payload",
                "invalid_request_error",
                None,
            );
        }
    };

    // 6. Pre-flight budget & BPE token count evaluation
    let tiktoken_messages: Vec<ChatCompletionRequestMessage> = request
        .messages
        .iter()
        .map(|msg| {
            let content_str = match &msg.content {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(val) => Some(val.to_string()),
                None => None,
            };
            ChatCompletionRequestMessage {
                role: msg.role.clone(),
                content: content_str,
                name: msg.name.clone(),
                function_call: None,
                tool_calls: Vec::new(),
                refusal: None,
            }
        })
        .collect();

    let prompt_tokens = match num_tokens_from_messages(&request.model, &tiktoken_messages) {
        Ok(t) => t,
        Err(_) => {
            // Fallback to standard gpt-4o tokenization
            num_tokens_from_messages("gpt-4o", &tiktoken_messages).unwrap_or(0)
        }
    };

    // Update total tokens consumed globally (prompt tokens first)
    state.total_tokens_consumed.fetch_add(prompt_tokens, Ordering::Relaxed);

    let pricing = get_model_pricing(&request.model);
    let prompt_cost = prompt_tokens as f64 * pricing.input_cost_per_token;

    let current_spend = {
        let map = state.spend_tracker.read().unwrap();
        map.get(&user_id).cloned().unwrap_or(0.0)
    };

    let projected_spend = current_spend + prompt_cost;

    if projected_spend >= state.default_budget {
        warn!(
            user_id = %user_id,
            current_spend = %current_spend,
            prompt_cost = %prompt_cost,
            projected_spend = %projected_spend,
            budget_limit = %state.default_budget,
            "Bankruptcy Shield tripped pre-flight: Projected spend exceeds budget limit"
        );
        state.record_request(&user_id, &request.model, 429, start_time.elapsed().as_millis() as u64);
        return make_error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Budget Exceeded",
            "requests",
            Some("budget_exceeded"),
        );
    }

    // Charge the user upfront for prompt tokens
    let total_spend = {
        let mut map = state.spend_tracker.write().unwrap();
        let entry = map.entry(user_id.clone()).or_insert(0.0);
        *entry += prompt_cost;
        *entry
    };
    info!(
        user_id = %user_id,
        prompt_tokens = %prompt_tokens,
        prompt_cost = %prompt_cost,
        total_spend = %total_spend,
        "Bankruptcy Shield: Charged prompt tokens pre-flight"
    );

    // Refund helper in case the upstream request fails
    let refund_prompt_cost = || {
        let mut map = state.spend_tracker.write().unwrap();
        if let Some(val) = map.get_mut(&user_id) {
            *val -= prompt_cost;
            if *val < 0.0 {
                *val = 0.0;
            }
            info!(
                user_id = %user_id,
                refunded = %prompt_cost,
                new_spend = %val,
                "Bankruptcy Shield: Refunded prompt tokens due to upstream error"
            );
        }
    };

    // 7. Conditional routing: Route to local mock if X-Mock-Upstream header is present
    let upstream_url = if headers.contains_key("x-mock-upstream") {
        info!("Routing to local mock upstream endpoint");
        format!("http://127.0.0.1:{}/mock/v1/chat/completions", state.port)
    } else {
        "https://api.openai.com/v1/chat/completions".to_string()
    };

    // 8. Prepare Upstream Request. We forward the exact body bytes.
    let upstream_req = state.client
        .post(&upstream_url)
        .header(reqwest::header::AUTHORIZATION, auth_val)
        .header(reqwest::header::CONTENT_TYPE, content_type_val)
        .body(body_bytes.clone());

    info!("Initiating handshake with upstream provider at {}...", upstream_url);
    let upstream_res = match upstream_req.send().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to connect to upstream: {:?}", e);
            refund_prompt_cost();
            state.record_request(&user_id, &request.model, 502, start_time.elapsed().as_millis() as u64);
            return make_error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Upstream connection failed: {}", e),
                "api_error",
                None,
            );
        }
    };

    let status = upstream_res.status();
    info!("Upstream handshake completed with status: {}", status);

    // 9. Handle non-2xx status codes by proxying the exact status and body payload back
    if !status.is_success() {
        refund_prompt_cost();
        let headers_clone = upstream_res.headers().clone();

        let error_bytes = match upstream_res.bytes().await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to read upstream error body: {:?}", e);
                state.record_request(&user_id, &request.model, 502, start_time.elapsed().as_millis() as u64);
                return make_error_response(
                    StatusCode::BAD_GATEWAY,
                    "Failed to read error details from upstream",
                    "api_error",
                    None,
                );
            }
        };

        warn!(
            "Upstream returned error status {}. Forwarding response body ({} bytes)",
            status,
            error_bytes.len()
        );

        state.record_request(&user_id, &request.model, status.as_u16(), start_time.elapsed().as_millis() as u64);

        let mut builder = Response::builder().status(status);
        if let Some(content_type) = headers_clone.get(axum::http::header::CONTENT_TYPE) {
            builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
        } else {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        }

        return builder.body(Body::from(error_bytes)).unwrap_or_else(|err| {
            error!("Failed to build error response: {:?}", err);
            make_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error forwarding response",
                "api_error",
                None,
            )
        });
    }

    // 10. Success flow: Stream the response back dynamically
    info!("Upstream request succeeded. Initiating downstream streaming...");

    let mut response_builder = Response::builder().status(StatusCode::OK);
    if let Some(ct) = upstream_res.headers().get(axum::http::header::CONTENT_TYPE) {
        response_builder = response_builder.header(axum::http::header::CONTENT_TYPE, ct);
    }
    if let Some(cc) = upstream_res.headers().get(axum::http::header::CACHE_CONTROL) {
        response_builder = response_builder.header(axum::http::header::CACHE_CONTROL, cc);
    }

    // Capture the bytes stream and wrap it with our StreamMonitor to track metrics and cancellations
    let raw_stream: BoxedByteStream = Box::pin(upstream_res.bytes_stream());
    let monitored_stream = StreamMonitor::new(
        raw_stream,
        user_id,
        state.spend_tracker.clone(),
        request.model,
        pricing,
        prompt_cost,
        total_spend,
        state.default_budget,
        state.clone(), // Pass state to enable stats updates on close/cancel
    );

    // Map the stream back to axum::Error to build the Axum response Body
    let mapped_stream = monitored_stream.map(|res| res.map_err(|e| axum::Error::new(e)));

    let body = Body::from_stream(mapped_stream);

    response_builder.body(body).unwrap_or_else(|err| {
        error!("Failed to build stream response: {:?}", err);
        make_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error starting stream",
            "api_error",
            None,
        )
    })
}

// Struct for dashboard and stats API payloads
#[derive(serde::Serialize, Clone)]
struct RecentRequest {
    timestamp: String,
    user_id: String,
    model: String,
    status: u16,
    duration_ms: u64,
}

#[derive(serde::Serialize)]
struct StatsPayload {
    health: HealthStats,
    budget: BudgetStats,
}

#[derive(serde::Serialize)]
struct HealthStats {
    uptime_seconds: u64,
    memory_usage_kb: usize,
    avg_latency_ms: f64,
}

#[derive(serde::Serialize)]
struct BudgetStats {
    total_tokens_consumed: usize,
    default_budget_usd: f64,
    recent_requests: Vec<RecentRequest>,
    current_spend_by_user: HashMap<String, f64>,
}

/// Helper function to retrieve RSS memory usage of the current process on Linux.
fn get_memory_usage_kb() -> usize {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<usize>() {
                            return kb;
                        }
                    }
                }
            }
        }
    }
    // Fallback/mock RSS memory usage (e.g. 15MB) when running locally on macOS
    15360
}

/// REST endpoint `/api/stats` to expose server telemetry and budget state.
async fn get_stats(State(state): State<AppState>) -> Response {
    let uptime = state.start_time.elapsed().as_secs();
    let memory_usage = get_memory_usage_kb();

    let total_reqs = state.total_requests.load(Ordering::Relaxed);
    let total_lat = state.total_latency_ms.load(Ordering::Relaxed);
    let avg_latency = if total_reqs > 0 {
        total_lat as f64 / total_reqs as f64
    } else {
        0.0
    };

    let recent = {
        let list = state.recent_requests.lock().unwrap();
        list.iter().cloned().collect::<Vec<RecentRequest>>()
    };

    let ledger = {
        let map = state.spend_tracker.read().unwrap();
        map.clone()
    };

    let payload = StatsPayload {
        health: HealthStats {
            uptime_seconds: uptime,
            memory_usage_kb: memory_usage,
            avg_latency_ms: avg_latency,
        },
        budget: BudgetStats {
            total_tokens_consumed: state.total_tokens_consumed.load(Ordering::Relaxed),
            default_budget_usd: state.default_budget,
            recent_requests: recent,
            current_spend_by_user: ledger,
        },
    };

    (StatusCode::OK, Json(payload)).into_response()
}

/// Route handler to render the embedded HTML dashboard.
async fn get_dashboard() -> impl IntoResponse {
    Html(DASHBOARD_HTML)
}

// Embedded dashboard HTML template using Tailwind CSS via CDN and vanilla JS polling
const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en" class="h-full bg-slate-950 text-slate-100">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Kilovolt Dashboard ⚡</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        tailwind.config = {
            theme: {
                extend: {
                    colors: {
                        brand: {
                            50: '#fefcf0',
                            100: '#fdf7d5',
                            500: '#eab308',
                            900: '#713f12',
                        }
                    }
                }
            }
        }
    </script>
</head>
<body class="min-h-full flex flex-col font-sans">
    <header class="border-b border-slate-800 bg-slate-900/50 backdrop-blur-md sticky top-0 z-50">
        <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
            <div class="flex items-center space-x-3">
                <span class="text-2xl">⚡</span>
                <span class="text-xl font-bold tracking-tight bg-gradient-to-r from-yellow-400 to-amber-500 bg-clip-text text-transparent">Kilovolt Admin</span>
            </div>
            <div class="flex items-center space-x-2">
                <span id="status-dot" class="h-2.5 w-2.5 rounded-full bg-green-500 animate-pulse"></span>
                <span id="status-text" class="text-xs text-slate-400 font-medium">Live</span>
            </div>
        </div>
    </header>

    <main class="flex-grow max-w-7xl w-full mx-auto px-4 sm:px-6 lg:px-8 py-8 space-y-8">
        <!-- Stats Overview Grid -->
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
            <!-- Card: System Health -->
            <div class="bg-slate-900/60 border border-slate-800 rounded-2xl p-6 shadow-xl backdrop-blur-sm hover:border-slate-700 transition duration-300">
                <div class="flex items-center justify-between mb-6">
                    <h2 class="text-lg font-semibold text-slate-200 flex items-center space-x-2">
                        <span>🖥️</span>
                        <span>System Health</span>
                    </h2>
                    <span class="text-xs bg-slate-800 text-slate-400 px-2.5 py-1 rounded-full font-mono">Metrics</span>
                </div>
                <div class="grid grid-cols-2 gap-4">
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Uptime</p>
                        <p id="uptime" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Memory RSS</p>
                        <p id="memory" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850 col-span-2">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Average Latency</p>
                        <p id="latency" class="text-2xl font-black text-amber-400 mt-1 font-mono">-</p>
                    </div>
                </div>
            </div>

            <!-- Card: Budget Pipeline -->
            <div class="bg-slate-900/60 border border-slate-800 rounded-2xl p-6 shadow-xl backdrop-blur-sm hover:border-slate-700 transition duration-300">
                <div class="flex items-center justify-between mb-6">
                    <h2 class="text-lg font-semibold text-slate-200 flex items-center space-x-2">
                        <span>🛡️</span>
                        <span>Budget Pipeline</span>
                    </h2>
                    <span class="text-xs bg-slate-800 text-slate-400 px-2.5 py-1 rounded-full font-mono">Ledger</span>
                </div>
                <div class="grid grid-cols-2 gap-4">
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Total Tokens</p>
                        <p id="total-tokens" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Default Budget</p>
                        <p id="default-budget" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850 col-span-2">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Active Users Ledger</p>
                        <div id="ledger-list" class="mt-2 space-y-1.5 max-h-24 overflow-y-auto text-sm">
                            <p class="text-slate-500 text-xs italic">No active users yet.</p>
                        </div>
                    </div>
                </div>
            </div>
        </div>

        <!-- Recent Logs / Requests -->
        <div class="bg-slate-900/40 border border-slate-800 rounded-2xl p-6 shadow-xl">
            <h2 class="text-lg font-semibold text-slate-200 mb-4 flex items-center space-x-2">
                <span>📋</span>
                <span>Recent Proxy Transactions</span>
            </h2>
            <div class="overflow-x-auto">
                <table class="min-w-full divide-y divide-slate-800 text-sm">
                    <thead>
                        <tr class="text-slate-400 font-medium text-left">
                            <th class="py-3 px-4">Time</th>
                            <th class="py-3 px-4">User ID</th>
                            <th class="py-3 px-4">Model</th>
                            <th class="py-3 px-4">Status</th>
                            <th class="py-3 px-4">Latency</th>
                        </tr>
                    </thead>
                    <tbody id="recent-requests-table" class="divide-y divide-slate-800/60 text-slate-300 font-mono">
                        <tr>
                            <td colspan="5" class="py-4 text-center text-slate-500 italic">Waiting for traffic...</td>
                        </tr>
                    </tbody>
                </table>
            </div>
        </div>
    </main>

    <footer class="border-t border-slate-900 bg-slate-950/80 py-4 text-center text-xs text-slate-600">
        Kilovolt Reverse Proxy Engine &copy; 2026. Made with Rust and Async speed.
    </footer>

    <script>
        function formatUptime(seconds) {
            const h = Math.floor(seconds / 3600);
            const m = Math.floor((seconds % 3600) / 60);
            const s = seconds % 60;
            return `${h}h ${m}m ${s}s`;
        }

        async function fetchStats() {
            try {
                const response = await fetch('/api/stats');
                if (!response.ok) throw new Error('API down');
                const data = await response.json();

                // System Health Updates
                document.getElementById('uptime').innerText = formatUptime(data.health.uptime_seconds);
                document.getElementById('memory').innerText = `${(data.health.memory_usage_kb / 1024).toFixed(2)} MB`;
                document.getElementById('latency').innerText = `${data.health.avg_latency_ms.toFixed(2)} ms`;

                // Budget Pipeline Updates
                document.getElementById('total-tokens').innerText = data.budget.total_tokens_consumed.toLocaleString();
                document.getElementById('default-budget').innerText = `$${data.budget.default_budget_usd.toFixed(4)}`;

                // Render ledger
                const ledgerList = document.getElementById('ledger-list');
                ledgerList.innerHTML = '';
                const users = Object.entries(data.budget.current_spend_by_user);
                if (users.length === 0) {
                    ledgerList.innerHTML = '<p class="text-slate-500 text-xs italic">No active users yet.</p>';
                } else {
                    users.forEach(([user, spend]) => {
                        const isOver = spend >= data.budget.default_budget_usd;
                        const statusClass = isOver ? 'text-red-400 font-bold' : 'text-green-400';
                        ledgerList.innerHTML += `
                            <div class="flex justify-between items-center bg-slate-950/80 px-3 py-1 rounded border border-slate-800/40">
                                <span class="font-medium text-slate-400">${user}</span>
                                <span class="${statusClass}">$${spend.toFixed(5)}</span>
                            </div>
                        `;
                    });
                }

                // Render recent requests
                const tableBody = document.getElementById('recent-requests-table');
                tableBody.innerHTML = '';
                if (data.budget.recent_requests.length === 0) {
                    tableBody.innerHTML = '<tr><td colspan="5" class="py-4 text-center text-slate-500 italic">Waiting for traffic...</td></tr>';
                } else {
                    data.budget.recent_requests.forEach(req => {
                        const statusClass = req.status >= 400 ? 'text-red-400' : 'text-green-400';
                        tableBody.innerHTML += `
                            <tr class="hover:bg-slate-900/30 transition">
                                <td class="py-3 px-4 text-slate-500">${req.timestamp}</td>
                                <td class="py-3 px-4 font-bold text-slate-300">${req.user_id}</td>
                                <td class="py-3 px-4 text-slate-400">${req.model}</td>
                                <td class="py-3 px-4"><span class="px-2 py-0.5 rounded text-xs font-bold ${statusClass} bg-slate-950 border border-slate-800">${req.status}</span></td>
                                <td class="py-3 px-4 text-amber-500 font-semibold">${req.duration_ms} ms</td>
                            </tr>
                        `;
                    });
                }

                // Status Dot indicator
                document.getElementById('status-dot').className = 'h-2.5 w-2.5 rounded-full bg-green-500 animate-pulse';
                document.getElementById('status-text').innerText = 'Live';
            } catch (err) {
                console.error(err);
                document.getElementById('status-dot').className = 'h-2.5 w-2.5 rounded-full bg-red-500 animate-ping';
                document.getElementById('status-text').innerText = 'Disconnected';
            }
        }

        // Poll every 3 seconds
        setInterval(fetchStats, 3000);
        // Initial load
        fetchStats();
    </script>
</body>
</html>"#;

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
