use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::stream::{self, StreamExt};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tiktoken_rs::{ChatCompletionRequestMessage, bpe_for_model, num_tokens_from_messages};
use tracing::{error, info, warn};

use crate::budget::{StreamMonitor, get_model_pricing};
use crate::config::AppState;
use crate::ledger::{BudgetError, BudgetScope, BudgetSnapshot};

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
pub fn make_error_response(
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
    (status, axum::Json(err)).into_response()
}

/// Deterministic local mock upstream used by tests and the benchmark harness.
pub async fn mock_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    info!("Handling mock chat completions upstream request");
    let body = match axum::body::to_bytes(body, state.max_request_body_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return make_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Mock request body exceeded KILOVOLT_MAX_REQUEST_BODY_BYTES",
                "invalid_request_error",
                Some("request_body_too_large"),
            );
        }
    };
    let request = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(request) => request,
        Err(_) => {
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Invalid mock request JSON",
                "invalid_request_error",
                None,
            );
        }
    };
    let streaming = request
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if !streaming {
        let response = serde_json::json!({
            "id": "chatcmpl-kilovolt-mock",
            "object": "chat.completion",
            "model": request.get("model").and_then(serde_json::Value::as_str).unwrap_or("mock"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "deterministic mock response"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
        });
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            response.to_string(),
        )
            .into_response();
    }

    let event_count = headers
        .get("x-mock-events")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 10_000);
    let delay_ms = headers
        .get("x-mock-delay-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .min(10_000);
    let stream = stream::unfold(0, move |index| async move {
        if index > event_count {
            return None;
        }
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        let frame = if index == event_count {
            Bytes::from_static(b"data: [DONE]\n\n")
        } else {
            Bytes::from(format!(
                "data: {{\"id\":\"chatcmpl-kilovolt-mock\",\"object\":\"chat.completion.chunk\",\"model\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\" token-{index}\"}},\"finish_reason\":null}}]}}\n\n"
            ))
        };
        Some((Ok::<Bytes, std::io::Error>(frame), index + 1))
    });

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache");

    builder.body(Body::from_stream(stream)).unwrap()
}

type BoxedByteStream =
    Pin<Box<dyn futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

struct PromptReservationGuard {
    state: AppState,
    request_id: String,
    user_id: String,
    model: String,
    prompt_tokens: usize,
    start_time: Instant,
    active: bool,
}

impl PromptReservationGuard {
    fn new(
        state: AppState,
        request_id: &str,
        user_id: &str,
        model: &str,
        prompt_tokens: usize,
        start_time: Instant,
    ) -> Self {
        Self {
            state,
            request_id: request_id.to_string(),
            user_id: user_id.to_string(),
            model: model.to_string(),
            prompt_tokens,
            start_time,
            active: true,
        }
    }

    fn commit(&mut self) -> Result<BudgetSnapshot, BudgetError> {
        let result = self
            .state
            .budget_ledger
            .commit_prompt(&self.request_id, &self.user_id);
        if result.is_ok() {
            self.active = false;
        }
        result
    }

    fn release(&mut self, reason: &str) {
        match self
            .state
            .budget_ledger
            .release_prompt(&self.request_id, &self.user_id)
        {
            Ok(snapshot) => info!(
                user_id = %self.user_id,
                request_id = %self.request_id,
                project_total_spend = %snapshot.project.total_spend,
                user_total_spend = %snapshot.user.total_spend,
                reason = %reason,
                "Bankruptcy Shield: Released prompt reservation"
            ),
            Err(error) => error!(
                user_id = %self.user_id,
                request_id = %self.request_id,
                reason = %reason,
                error = %error,
                "Failed to release prompt reservation"
            ),
        }
        self.active = false;
    }
}

impl Drop for PromptReservationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let release_result = self
            .state
            .budget_ledger
            .release_prompt(&self.request_id, &self.user_id);
        if let Err(error) = release_result {
            error!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                error = %error,
                "Failed to release prompt reservation after request task cancellation"
            );
        }
        self.state.record_request(
            &self.request_id,
            &self.user_id,
            &self.model,
            499,
            self.start_time.elapsed().as_millis() as u64,
            self.prompt_tokens,
            0.0,
        );
    }
}

#[derive(Debug)]
enum BoundedBodyError {
    TooLarge,
    Upstream(reqwest::Error),
}

async fn read_bounded_response(
    response: reqwest::Response,
    limit: usize,
) -> Result<Bytes, BoundedBodyError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(BoundedBodyError::Upstream)?;
        let new_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(BoundedBodyError::TooLarge)?;
        if new_len > limit {
            return Err(BoundedBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn content_type_starts_with(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with(expected))
}

fn fallback_message_text(value: &serde_json::Value) -> Option<String> {
    let choices = value.get("choices")?.as_array()?;
    let mut output = String::new();
    for choice in choices {
        let content = choice
            .get("message")
            .and_then(|message| message.get("content"));
        match content {
            Some(serde_json::Value::String(text)) => output.push_str(text),
            Some(serde_json::Value::Array(parts)) => {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        output.push_str(text);
                    }
                }
            }
            Some(serde_json::Value::Null) | None => {}
            Some(_) => return None,
        }
    }
    Some(output)
}

fn non_stream_output_tokens(
    model: &str,
    value: &serde_json::Value,
) -> Result<(usize, &'static str), &'static str> {
    if let Some(tokens) = value
        .get("usage")
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(serde_json::Value::as_u64)
    {
        let tokens = usize::try_from(tokens).map_err(|_| "provider usage was too large")?;
        return Ok((tokens, "provider usage.completion_tokens"));
    }

    let output = fallback_message_text(value)
        .ok_or("response had neither supported usage nor completion message content")?;
    let tokens = bpe_for_model(model)
        .ok()
        .or_else(|| bpe_for_model("gpt-4o").ok())
        .map_or_else(
            || output.len().div_ceil(4),
            |bpe| bpe.encode_with_special_tokens(&output).len(),
        );
    Ok((tokens, "local complete-message tokenizer fallback"))
}

fn record_token_budget_usage(state: &AppState, pipeline_id: Option<&str>, total_tokens: usize) {
    state
        .tokens_used_today
        .fetch_add(total_tokens, Ordering::Relaxed);
    if let Some(pipeline_id) = pipeline_id {
        let mut tracker = state.pipeline_tracker.write().unwrap();
        *tracker.entry(pipeline_id.to_string()).or_insert(0) += total_tokens;
    }
}

/// The core chat completions reverse proxy endpoint handler.
pub async fn chat_completions_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    info!("Ingesting POST /v1/chat/completions request");
    let start_time = Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();

    // 1. Extract and validate Authorization header
    let auth_val = match headers.get(axum::http::header::AUTHORIZATION) {
        Some(val) => {
            let val_str = match val.to_str() {
                Ok(s) => s,
                Err(_) => {
                    warn!("Invalid UTF-8 in Authorization header");
                    state.record_request(
                        &request_id,
                        "anonymous",
                        "unknown",
                        401,
                        start_time.elapsed().as_millis() as u64,
                        0,
                        0.0,
                    );
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
                state.record_request(
                    &request_id,
                    "anonymous",
                    "unknown",
                    401,
                    start_time.elapsed().as_millis() as u64,
                    0,
                    0.0,
                );
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
            state.record_request(
                &request_id,
                "anonymous",
                "unknown",
                401,
                start_time.elapsed().as_millis() as u64,
                0,
                0.0,
            );
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
                    state.record_request(
                        &request_id,
                        "anonymous",
                        "unknown",
                        400,
                        start_time.elapsed().as_millis() as u64,
                        0,
                        0.0,
                    );
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
                state.record_request(
                    &request_id,
                    "anonymous",
                    "unknown",
                    400,
                    start_time.elapsed().as_millis() as u64,
                    0,
                    0.0,
                );
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
            state.record_request(
                &request_id,
                "anonymous",
                "unknown",
                400,
                start_time.elapsed().as_millis() as u64,
                0,
                0.0,
            );
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
    let body_bytes = match axum::body::to_bytes(body, state.max_request_body_bytes).await {
        Ok(b) => b,
        Err(e) => {
            warn!(
                max_request_body_bytes = %state.max_request_body_bytes,
                error = ?e,
                "Request body exceeded the configured bound or could not be read"
            );
            state.record_request(
                &request_id,
                &user_id,
                "unknown",
                413,
                start_time.elapsed().as_millis() as u64,
                0,
                0.0,
            );
            return make_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeds KILOVOLT_MAX_REQUEST_BODY_BYTES",
                "invalid_request_error",
                Some("request_body_too_large"),
            );
        }
    };

    // 5. Parse request body JSON
    let request: IncomingRequest = match serde_json::from_slice(&body_bytes) {
        Ok(req) => req,
        Err(e) => {
            warn!("Failed to deserialize request JSON: {:?}", e);
            state.record_request(
                &request_id,
                &user_id,
                "unknown",
                400,
                start_time.elapsed().as_millis() as u64,
                0,
                0.0,
            );
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Invalid JSON payload",
                "invalid_request_error",
                None,
            );
        }
    };

    let is_gemini = request.model.starts_with("gemini-");
    if is_gemini && !request.stream {
        state.record_request(
            &request_id,
            &user_id,
            &request.model,
            400,
            start_time.elapsed().as_millis() as u64,
            0,
            0.0,
        );
        return make_error_response(
            StatusCode::BAD_REQUEST,
            "Non-streaming Gemini translation is not supported; set stream=true",
            "invalid_request_error",
            Some("unsupported_response_mode"),
        );
    }

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

    // 6.5. Pre-flight check for multi-tier token budgeting
    let pipeline_id = headers
        .get("X-Pipeline-ID")
        .and_then(|val| val.to_str().ok())
        .map(|s| s.to_string());
    let pipeline_name = headers
        .get("X-Pipeline-Name")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("unknown-pipeline");
    let step_name = headers
        .get("X-Step-Name")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("unknown-step");

    if let Err(err_msg) =
        crate::budget::check_token_budgets(&state, pipeline_id.as_deref(), prompt_tokens)
    {
        warn!(
            pipeline = %pipeline_name,
            step = %step_name,
            error = %err_msg,
            "[pipeline:{}][step:{}] {}", pipeline_name, step_name, err_msg
        );
        state.record_request(
            &request_id,
            &user_id,
            &request.model,
            429,
            start_time.elapsed().as_millis() as u64,
            prompt_tokens,
            0.0,
        );
        return make_error_response(
            StatusCode::TOO_MANY_REQUESTS,
            &err_msg,
            "requests",
            Some("budget_exceeded"),
        );
    }

    let pricing = get_model_pricing(&request.model);
    let prompt_cost = prompt_tokens as f64 * pricing.input_cost_per_token;

    let reservation = match state.budget_ledger.reserve_prompt(
        &request_id,
        &user_id,
        prompt_cost,
        state.project_budget,
        state.default_budget,
    ) {
        Ok(snapshot) => snapshot,
        Err(error @ BudgetError::BudgetExceeded(scope)) => {
            let user_snapshot = state.budget_ledger.user_snapshot(&user_id);
            let project_snapshot = state.budget_ledger.project_snapshot();
            let response_message = match scope {
                BudgetScope::Project => "Project Budget Exceeded",
                BudgetScope::User => "User Budget Exceeded",
            };
            warn!(
                user_id = %user_id,
                budget_scope = %scope,
                current_project_total_spend = %project_snapshot.total_spend,
                current_user_total_spend = %user_snapshot.total_spend,
                prompt_cost = %prompt_cost,
                project_budget_limit = %state.project_budget,
                user_budget_limit = %state.default_budget,
                "Bankruptcy Shield tripped pre-flight: {}", error
            );
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                429,
                start_time.elapsed().as_millis() as u64,
                prompt_tokens,
                0.0,
            );
            return make_error_response(
                StatusCode::TOO_MANY_REQUESTS,
                response_message,
                "requests",
                Some("budget_exceeded"),
            );
        }
        Err(error) => {
            error!(
                user_id = %user_id,
                request_id = %request_id,
                error = %error,
                "Failed to create prompt budget reservation"
            );
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                500,
                start_time.elapsed().as_millis() as u64,
                prompt_tokens,
                0.0,
            );
            return make_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal budget ledger error",
                "api_error",
                None,
            );
        }
    };

    info!(
        user_id = %user_id,
        request_id = %request_id,
        prompt_tokens = %prompt_tokens,
        prompt_cost = %prompt_cost,
        project_total_spend_with_reservations = %reservation.project.total_spend,
        user_total_spend_with_reservations = %reservation.user.total_spend,
        "Bankruptcy Shield: Reserved prompt cost"
    );
    let mut reservation_guard = PromptReservationGuard::new(
        state.clone(),
        &request_id,
        &user_id,
        &request.model,
        prompt_tokens,
        start_time,
    );

    // Update total tokens consumed globally after the financial reservation succeeds.
    state
        .total_tokens_consumed
        .fetch_add(prompt_tokens, Ordering::Relaxed);

    // Extract raw API Key for downstream delivery
    let api_key = auth_val
        .to_str()
        .unwrap_or("")
        .trim_start_matches("Bearer ")
        .to_string();

    // 7. Conditional routing: Route to local mock if X-Mock-Upstream header is present
    let upstream_url = if headers.contains_key("x-mock-upstream") {
        info!("Routing to local mock upstream endpoint");
        format!("http://127.0.0.1:{}/mock/v1/chat/completions", state.port)
    } else if is_gemini {
        // Route to Gemini native API
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            request.model
        )
    } else {
        state.openai_upstream_url.clone()
    };

    // 8. Prepare Upstream Request. We format payload according to provider targets.
    let mut upstream_req = state.client.post(&upstream_url);
    if headers.contains_key("x-mock-upstream") {
        for name in ["x-mock-events", "x-mock-delay-ms"] {
            if let Some(value) = headers.get(name) {
                upstream_req = upstream_req.header(name, value);
            }
        }
    }

    if is_gemini && !headers.contains_key("x-mock-upstream") {
        // Gemini Native SSE parameters & body mapping
        upstream_req = upstream_req
            .header("x-goog-api-key", &api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json");

        #[derive(serde::Serialize)]
        struct GeminiRequest {
            contents: Vec<GeminiContent>,
        }
        #[derive(serde::Serialize)]
        struct GeminiContent {
            role: String,
            parts: Vec<GeminiPart>,
        }
        #[derive(serde::Serialize)]
        struct GeminiPart {
            text: String,
        }

        let gemini_contents: Vec<GeminiContent> = request
            .messages
            .iter()
            .map(|msg| {
                let role = match msg.role.as_str() {
                    "assistant" => "model",
                    r => r,
                }
                .to_string();

                let content_str = match &msg.content {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(val) => val.to_string(),
                    None => "".to_string(),
                };

                GeminiContent {
                    role,
                    parts: vec![GeminiPart { text: content_str }],
                }
            })
            .collect();

        let gemini_req = GeminiRequest {
            contents: gemini_contents,
        };
        let gemini_body = match serde_json::to_vec(&gemini_req) {
            Ok(body) => body,
            Err(error) => {
                reservation_guard.release("Gemini request serialization failed");
                error!("Failed to serialize Gemini request: {:?}", error);
                return make_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to prepare upstream request",
                    "api_error",
                    None,
                );
            }
        };
        upstream_req = upstream_req.body(gemini_body);
    } else {
        // Standard OpenAI layout
        upstream_req = upstream_req
            .header(reqwest::header::AUTHORIZATION, auth_val)
            .header(reqwest::header::CONTENT_TYPE, content_type_val)
            .body(body_bytes.clone());
    }

    info!(
        "Initiating handshake with upstream provider at {}...",
        upstream_url
    );
    let upstream_res =
        match tokio::time::timeout(state.upstream_header_timeout, upstream_req.send()).await {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                error!("Failed to connect to upstream: {:?}", e);
                reservation_guard.release("upstream connection failed");
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    502,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    0.0,
                );
                return make_error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("Upstream connection failed: {}", e),
                    "api_error",
                    None,
                );
            }
            Err(_) => {
                error!(
                    timeout_seconds = %state.upstream_header_timeout.as_secs(),
                    "Timed out waiting for upstream response headers"
                );
                reservation_guard.release("upstream response-header timeout");
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    504,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    0.0,
                );
                return make_error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Timed out waiting for upstream response headers",
                    "api_error",
                    Some("upstream_timeout"),
                );
            }
        };

    let status = upstream_res.status();
    info!("Upstream handshake completed with status: {}", status);

    // 9. Handle non-2xx status codes by proxying the exact status and body payload back
    if !status.is_success() {
        reservation_guard.release("upstream returned non-success status");
        let headers_clone = upstream_res.headers().clone();

        let error_bytes =
            match read_bounded_response(upstream_res, state.max_upstream_body_bytes).await {
                Ok(body) => body,
                Err(BoundedBodyError::TooLarge) => {
                    error!(
                        max_upstream_body_bytes = %state.max_upstream_body_bytes,
                        "Upstream error body exceeded the configured bound"
                    );
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        prompt_tokens,
                        0.0,
                    );
                    return make_error_response(
                        StatusCode::BAD_GATEWAY,
                        "Upstream error body exceeded KILOVOLT_MAX_UPSTREAM_BODY_BYTES",
                        "api_error",
                        Some("upstream_body_too_large"),
                    );
                }
                Err(BoundedBodyError::Upstream(error)) => {
                    error!("Failed to read upstream error body: {:?}", error);
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        prompt_tokens,
                        0.0,
                    );
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

        state.record_request(
            &request_id,
            &user_id,
            &request.model,
            status.as_u16(),
            start_time.elapsed().as_millis() as u64,
            prompt_tokens,
            0.0,
        );

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

    let committed = match reservation_guard.commit() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            error!(
                user_id = %user_id,
                request_id = %request_id,
                error = %error,
                "Failed to commit prompt reservation after upstream acceptance"
            );
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                500,
                start_time.elapsed().as_millis() as u64,
                prompt_tokens,
                0.0,
            );
            return make_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal budget ledger error",
                "api_error",
                None,
            );
        }
    };

    // 10. Success flow: Stream the response back dynamically
    info!(
        user_id = %user_id,
        request_id = %request_id,
        prompt_cost = %prompt_cost,
        project_total_spend = %committed.project.total_spend,
        user_total_spend = %committed.user.total_spend,
        "Upstream request succeeded. Committed prompt reservation and initiating downstream streaming."
    );

    let upstream_headers = upstream_res.headers().clone();

    if !request.stream {
        if !content_type_starts_with(&upstream_headers, "application/json") {
            record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                502,
                start_time.elapsed().as_millis() as u64,
                prompt_tokens,
                prompt_cost,
            );
            return make_error_response(
                StatusCode::BAD_GATEWAY,
                "Non-streaming upstream response must use application/json",
                "api_error",
                Some("invalid_upstream_content_type"),
            );
        }

        let response_bytes =
            match read_bounded_response(upstream_res, state.max_upstream_body_bytes).await {
                Ok(body) => body,
                Err(BoundedBodyError::TooLarge) => {
                    record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        prompt_tokens,
                        prompt_cost,
                    );
                    return make_error_response(
                        StatusCode::BAD_GATEWAY,
                        "Upstream response exceeded KILOVOLT_MAX_UPSTREAM_BODY_BYTES",
                        "api_error",
                        Some("upstream_body_too_large"),
                    );
                }
                Err(BoundedBodyError::Upstream(error)) => {
                    error!("Upstream disconnected while reading JSON response: {error}");
                    record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        prompt_tokens,
                        prompt_cost,
                    );
                    return make_error_response(
                        StatusCode::BAD_GATEWAY,
                        "Upstream disconnected while reading the response",
                        "api_error",
                        Some("upstream_disconnect"),
                    );
                }
            };
        let response_json = match serde_json::from_slice::<serde_json::Value>(&response_bytes) {
            Ok(value) => value,
            Err(_) => {
                record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    502,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    prompt_cost,
                );
                return make_error_response(
                    StatusCode::BAD_GATEWAY,
                    "Non-streaming upstream response was not valid JSON",
                    "api_error",
                    Some("malformed_upstream_response"),
                );
            }
        };
        let (output_tokens, accounting_source) =
            match non_stream_output_tokens(&request.model, &response_json) {
                Ok(accounting) => accounting,
                Err(reason) => {
                    record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        prompt_tokens,
                        prompt_cost,
                    );
                    return make_error_response(
                        StatusCode::BAD_GATEWAY,
                        &format!("Unsupported non-streaming upstream response: {reason}"),
                        "api_error",
                        Some("malformed_upstream_response"),
                    );
                }
            };
        let output_cost = output_tokens as f64 * pricing.output_cost_per_token;
        let charged = match state.budget_ledger.try_charge_output(
            &user_id,
            output_cost,
            state.project_budget,
            state.default_budget,
        ) {
            Ok(snapshot) => snapshot,
            Err(BudgetError::BudgetExceeded(scope)) => {
                warn!(
                    user_id = %user_id,
                    budget_scope = %scope,
                    output_tokens = %output_tokens,
                    output_cost = %output_cost,
                    "Non-streaming output was withheld because its charge would exceed a budget"
                );
                record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    429,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    prompt_cost,
                );
                return make_error_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    match scope {
                        BudgetScope::Project => "Project Budget Exceeded",
                        BudgetScope::User => "User Budget Exceeded",
                    },
                    "requests",
                    Some("budget_exceeded"),
                );
            }
            Err(error) => {
                error!(
                    request_id = %request_id,
                    user_id = %user_id,
                    error = %error,
                    "Failed to charge non-streaming output"
                );
                record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    500,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    prompt_cost,
                );
                return make_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal budget ledger error",
                    "api_error",
                    None,
                );
            }
        };
        state
            .total_tokens_consumed
            .fetch_add(output_tokens, Ordering::Relaxed);
        let total_tokens = prompt_tokens + output_tokens;
        record_token_budget_usage(&state, pipeline_id.as_deref(), total_tokens);
        state.record_request(
            &request_id,
            &user_id,
            &request.model,
            status.as_u16(),
            start_time.elapsed().as_millis() as u64,
            total_tokens,
            prompt_cost + output_cost,
        );
        info!(
            request_id = %request_id,
            user_id = %user_id,
            output_tokens = %output_tokens,
            output_accounting_source = %accounting_source,
            project_total_spend = %charged.project.total_spend,
            user_total_spend = %charged.user.total_spend,
            "Forwarding accounted non-streaming response"
        );

        let mut builder = Response::builder().status(status);
        if let Some(content_type) = upstream_headers.get(axum::http::header::CONTENT_TYPE) {
            builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
        }
        return builder
            .body(Body::from(response_bytes))
            .unwrap_or_else(|error| {
                error!("Failed to build non-streaming response: {error}");
                make_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error forwarding response",
                    "api_error",
                    None,
                )
            });
    }

    if !content_type_starts_with(&upstream_headers, "text/event-stream") {
        record_token_budget_usage(&state, pipeline_id.as_deref(), prompt_tokens);
        state.record_request(
            &request_id,
            &user_id,
            &request.model,
            502,
            start_time.elapsed().as_millis() as u64,
            prompt_tokens,
            prompt_cost,
        );
        return make_error_response(
            StatusCode::BAD_GATEWAY,
            "Streaming upstream response must use text/event-stream",
            "api_error",
            Some("invalid_upstream_content_type"),
        );
    }

    let mut response_builder = Response::builder().status(status);
    if let Some(ct) = upstream_headers.get(axum::http::header::CONTENT_TYPE) {
        response_builder = response_builder.header(axum::http::header::CONTENT_TYPE, ct);
    }
    if let Some(cc) = upstream_headers.get(axum::http::header::CACHE_CONTROL) {
        response_builder = response_builder.header(axum::http::header::CACHE_CONTROL, cc);
    }

    // Capture the bytes stream and wrap it with our StreamMonitor to track metrics and cancellations
    let raw_stream: BoxedByteStream = Box::pin(upstream_res.bytes_stream());
    let monitored_stream = StreamMonitor::new(
        raw_stream,
        request_id,
        user_id,
        request.model,
        pricing,
        prompt_tokens,
        prompt_cost,
        committed.user.total_spend,
        state.default_budget,
        state.clone(), // Pass state to enable stats updates on close/cancel
        is_gemini,
        pipeline_id,
    );

    // Map the stream back to axum::Error to build the Axum response Body
    let mapped_stream = monitored_stream.map(|res| res.map_err(axum::Error::new));

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

#[cfg(test)]
mod tests {
    use super::chat_completions_proxy;
    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::routing::post;
    use futures_util::stream;
    use http_body_util::BodyExt;
    use std::time::Duration;
    use tiktoken_rs::{ChatCompletionRequestMessage, bpe_for_model, num_tokens_from_messages};

    use crate::budget::get_model_pricing;
    use crate::config::{AppState, test_state, test_state_with_budgets};

    fn proxy_headers(user_id: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer test-key"),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "x-user-id",
            HeaderValue::from_str(user_id).expect("test user ID should be a valid header"),
        );
        headers.insert("x-mock-upstream", HeaderValue::from_static("true"));
        headers
    }

    fn proxy_body() -> Body {
        proxy_body_with_stream(true)
    }

    fn proxy_body_with_stream(stream: bool) -> Body {
        Body::from(
            serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{"role": "user", "content": "test prompt"}],
                "stream": stream
            })
            .to_string(),
        )
    }

    async fn call_proxy(state: AppState, user_id: &str) -> axum::response::Response {
        chat_completions_proxy(State(state), proxy_headers(user_id), proxy_body()).await
    }

    async fn call_proxy_with_body(
        state: AppState,
        user_id: &str,
        body: Body,
    ) -> axum::response::Response {
        chat_completions_proxy(State(state), proxy_headers(user_id), body).await
    }

    fn proxy_prompt_cost() -> f64 {
        let messages = vec![ChatCompletionRequestMessage {
            role: "user".to_string(),
            content: Some("test prompt".to_string()),
            name: None,
            function_call: None,
            tool_calls: Vec::new(),
            refusal: None,
        }];
        let tokens = num_tokens_from_messages("gpt-4o-mini", &messages)
            .expect("test prompt should tokenize");
        tokens as f64 * get_model_pricing("gpt-4o-mini").input_cost_per_token
    }

    async fn spawn_mock_upstream(status: StatusCode) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock upstream should bind");
        let port = listener
            .local_addr()
            .expect("mock upstream should have a local address")
            .port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(move || async move {
                axum::response::Response::builder()
                    .status(status)
                    .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(if status.is_success() {
                        "data: [DONE]\n\n"
                    } else {
                        "{\"error\":\"upstream rejected request\"}"
                    }))
                    .expect("mock response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock upstream should serve");
        });
        (port, server)
    }

    async fn spawn_static_upstream(
        status: StatusCode,
        content_type: &'static str,
        body: &'static str,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(move || async move {
                axum::response::Response::builder()
                    .status(status)
                    .header(axum::http::header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .expect("mock response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock upstream should serve");
        });
        (port, server)
    }

    async fn response_bytes(response: axum::response::Response) -> Bytes {
        response
            .into_body()
            .collect()
            .await
            .expect("response body should collect")
            .to_bytes()
    }

    #[tokio::test]
    async fn upstream_connection_failure_releases_prompt_reservation() {
        let state = test_state(0, 1.0);

        let response = call_proxy(state.clone(), "test-user").await;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let snapshot = state.budget_ledger.user_snapshot("test-user");
        assert_eq!(snapshot.committed_spend, 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
        assert_eq!(snapshot.total_spend, 0.0);
    }

    #[tokio::test]
    async fn upstream_non_success_releases_prompt_reservation() {
        let (port, server) = spawn_mock_upstream(StatusCode::UNAUTHORIZED).await;
        let state = test_state(port, 1.0);

        let response = call_proxy(state.clone(), "test-user").await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let snapshot = state.budget_ledger.user_snapshot("test-user");
        assert_eq!(snapshot.committed_spend, 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
        assert_eq!(snapshot.total_spend, 0.0);
        server.abort();
    }

    #[tokio::test]
    async fn upstream_success_commits_prompt_reservation() {
        let (port, server) = spawn_mock_upstream(StatusCode::OK).await;
        let state = test_state(port, 1.0);

        let response = call_proxy(state.clone(), "test-user").await;

        assert_eq!(response.status(), StatusCode::OK);
        let snapshot = state.budget_ledger.user_snapshot("test-user");
        assert!(snapshot.committed_spend > 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
        assert_eq!(snapshot.total_spend, snapshot.committed_spend);

        drop(response);
        let after_disconnect = state.budget_ledger.user_snapshot("test-user");
        assert_eq!(after_disconnect.committed_spend, snapshot.committed_spend);
        server.abort();
    }

    #[tokio::test]
    async fn project_budget_blocks_a_second_user_without_changing_either_ledger() {
        let (port, server) = spawn_mock_upstream(StatusCode::OK).await;
        let prompt_cost = proxy_prompt_cost();
        let state = test_state_with_budgets(port, prompt_cost * 1.5, 1.0);

        let first_response = call_proxy(state.clone(), "user-a").await;
        assert_eq!(first_response.status(), StatusCode::OK);
        drop(first_response);

        let project_before = state.budget_ledger.project_snapshot();
        let user_a_before = state.budget_ledger.user_snapshot("user-a");
        let second_response = call_proxy(state.clone(), "user-b").await;

        assert_eq!(second_response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(state.budget_ledger.project_snapshot(), project_before);
        assert_eq!(state.budget_ledger.user_snapshot("user-a"), user_a_before);
        assert_eq!(state.budget_ledger.user_snapshot("user-b").total_spend, 0.0);
        server.abort();
    }

    #[tokio::test]
    async fn streaming_proxy_preserves_frames_charges_output_and_updates_dashboard_stats() {
        const UPSTREAM: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "text/event-stream", UPSTREAM).await;
        let state = test_state(port, 1.0);

        let response = call_proxy(state.clone(), "stream-user").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_bytes(response).await,
            Bytes::from_static(UPSTREAM.as_bytes())
        );

        let user = state.budget_ledger.user_snapshot("stream-user");
        assert!(user.committed_spend > proxy_prompt_cost());
        assert_eq!(user.reserved_spend, 0.0);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(200)
        );

        let mut dashboard_headers = HeaderMap::new();
        dashboard_headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer test-dashboard-token"),
        );
        let stats = crate::dashboard::get_stats(State(state.clone()), dashboard_headers).await;
        let stats_json: serde_json::Value =
            serde_json::from_slice(&response_bytes(stats).await).expect("stats should be JSON");
        assert_eq!(
            stats_json["budget"]["current_project_spend_usd"]
                .as_f64()
                .expect("project spend should be numeric"),
            state.budget_ledger.project_snapshot().committed_spend
        );
        server.abort();
    }

    #[tokio::test]
    async fn streaming_output_stops_before_user_budget_is_exceeded() {
        const FIRST: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        const UPSTREAM: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "text/event-stream", UPSTREAM).await;
        let pricing = get_model_pricing("gpt-4o-mini");
        let first_tokens = bpe_for_model("gpt-4o-mini")
            .unwrap()
            .encode_with_special_tokens("hello")
            .len();
        let limit = proxy_prompt_cost() + first_tokens as f64 * pricing.output_cost_per_token;
        let state = test_state_with_budgets(port, 1.0, limit);

        let output = response_bytes(call_proxy(state.clone(), "user-cutoff").await).await;
        assert_eq!(output, Bytes::from_static(FIRST.as_bytes()));
        let user = state.budget_ledger.user_snapshot("user-cutoff");
        assert!(user.total_spend <= limit);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(429)
        );
        server.abort();
    }

    #[tokio::test]
    async fn streaming_output_stops_before_project_budget_is_exceeded() {
        const FIRST: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        const UPSTREAM: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n"
        );
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "text/event-stream", UPSTREAM).await;
        let pricing = get_model_pricing("gpt-4o-mini");
        let first_tokens = bpe_for_model("gpt-4o-mini")
            .unwrap()
            .encode_with_special_tokens("hello")
            .len();
        let project_limit =
            proxy_prompt_cost() + first_tokens as f64 * pricing.output_cost_per_token;
        let state = test_state_with_budgets(port, project_limit, 1.0);

        let output = response_bytes(call_proxy(state.clone(), "project-cutoff").await).await;
        assert_eq!(output, Bytes::from_static(FIRST.as_bytes()));
        assert!(state.budget_ledger.project_snapshot().total_spend <= project_limit);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(429)
        );
        server.abort();
    }

    #[tokio::test]
    async fn non_streaming_response_uses_provider_usage_and_is_returned_intact() {
        const UPSTREAM: &str = "{\"id\":\"chatcmpl-test\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"hello\"}}],\"usage\":{\"prompt_tokens\":99,\"completion_tokens\":3,\"total_tokens\":102}}";
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "application/json", UPSTREAM).await;
        let state = test_state(port, 1.0);

        let response =
            call_proxy_with_body(state.clone(), "json-user", proxy_body_with_stream(false)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_bytes(response).await,
            Bytes::from_static(UPSTREAM.as_bytes())
        );
        let expected =
            proxy_prompt_cost() + 3.0 * get_model_pricing("gpt-4o-mini").output_cost_per_token;
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("json-user")
                .committed_spend,
            expected
        );
        server.abort();
    }

    #[tokio::test]
    async fn non_streaming_response_falls_back_to_complete_message_tokenization() {
        const UPSTREAM: &str =
            "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"hello world\"}}]}";
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "application/json; charset=utf-8", UPSTREAM)
                .await;
        let state = test_state(port, 1.0);

        let response = call_proxy_with_body(
            state.clone(),
            "fallback-user",
            proxy_body_with_stream(false),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_bytes(response).await,
            Bytes::from_static(UPSTREAM.as_bytes())
        );
        let expected_tokens = bpe_for_model("gpt-4o-mini")
            .unwrap()
            .encode_with_special_tokens("hello world")
            .len();
        let expected = proxy_prompt_cost()
            + expected_tokens as f64 * get_model_pricing("gpt-4o-mini").output_cost_per_token;
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("fallback-user")
                .committed_spend,
            expected
        );
        server.abort();
    }

    #[tokio::test]
    async fn non_streaming_output_is_withheld_when_its_charge_exceeds_budget() {
        const UPSTREAM: &str = "{\"choices\":[{\"message\":{\"content\":\"expensive\"}}],\"usage\":{\"completion_tokens\":1000}}";
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "application/json", UPSTREAM).await;
        let prompt_cost = proxy_prompt_cost();
        let state = test_state_with_budgets(port, 1.0, prompt_cost);

        let response =
            call_proxy_with_body(state.clone(), "json-cutoff", proxy_body_with_stream(false)).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("json-cutoff")
                .committed_spend,
            prompt_cost
        );
        server.abort();
    }

    #[tokio::test]
    async fn non_streaming_upstream_body_limit_is_enforced_after_prompt_commit() {
        const UPSTREAM: &str =
            "{\"choices\":[{\"message\":{\"content\":\"response larger than limit\"}}]}";
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "application/json", UPSTREAM).await;
        let mut state = test_state(port, 1.0);
        state.max_upstream_body_bytes = 16;

        let response =
            call_proxy_with_body(state.clone(), "large-json", proxy_body_with_stream(false)).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let user = state.budget_ledger.user_snapshot("large-json");
        assert!(user.committed_spend > 0.0);
        assert_eq!(user.reserved_spend, 0.0);
        server.abort();
    }

    #[tokio::test]
    async fn upstream_error_status_and_body_are_preserved_and_reservation_released() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let (port, server) =
                spawn_static_upstream(status, "application/json", "{\"error\":\"upstream\"}").await;
            let state = test_state(port, 1.0);
            let response = call_proxy(state.clone(), "error-user").await;
            assert_eq!(response.status(), status);
            assert_eq!(
                response_bytes(response).await,
                Bytes::from_static(b"{\"error\":\"upstream\"}")
            );
            assert_eq!(
                state.budget_ledger.user_snapshot("error-user").total_spend,
                0.0
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn invalid_success_content_types_and_malformed_json_are_rejected_after_acceptance() {
        let (stream_port, stream_server) =
            spawn_static_upstream(StatusCode::OK, "application/json", "{}").await;
        let stream_state = test_state(stream_port, 1.0);
        assert_eq!(
            call_proxy(stream_state.clone(), "stream-type")
                .await
                .status(),
            StatusCode::BAD_GATEWAY
        );
        assert!(
            stream_state
                .budget_ledger
                .user_snapshot("stream-type")
                .committed_spend
                > 0.0
        );
        stream_server.abort();

        let (json_port, json_server) =
            spawn_static_upstream(StatusCode::OK, "application/json", "{malformed").await;
        let json_state = test_state(json_port, 1.0);
        assert_eq!(
            call_proxy_with_body(
                json_state.clone(),
                "json-malformed",
                proxy_body_with_stream(false)
            )
            .await
            .status(),
            StatusCode::BAD_GATEWAY
        );
        assert!(
            json_state
                .budget_ledger
                .user_snapshot("json-malformed")
                .committed_spend
                > 0.0
        );
        json_server.abort();
    }

    #[tokio::test]
    async fn request_body_limit_accepts_below_and_exact_and_rejects_over_and_chunked() {
        let json = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "test prompt"}],
            "stream": true
        })
        .to_string();

        let mut below_state = test_state(0, 1.0);
        below_state.max_request_body_bytes = json.len() + 1;
        assert_ne!(
            call_proxy_with_body(below_state, "below-limit", Body::from(json.clone()))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let mut exact_state = test_state(0, 1.0);
        exact_state.max_request_body_bytes = json.len();
        assert_ne!(
            call_proxy_with_body(exact_state, "exact-limit", Body::from(json.clone()))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let mut over_state = test_state(0, 1.0);
        over_state.max_request_body_bytes = json.len() - 1;
        assert_eq!(
            call_proxy_with_body(over_state, "over-limit", Body::from(json.clone()))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let mut chunked_state = test_state(0, 1.0);
        chunked_state.max_request_body_bytes = json.len() - 1;
        let midpoint = json.len() / 2;
        let first = Bytes::copy_from_slice(&json.as_bytes()[..midpoint]);
        let second = Bytes::copy_from_slice(&json.as_bytes()[midpoint..]);
        let chunked = Body::from_stream(stream::iter(vec![
            Ok::<Bytes, std::io::Error>(first),
            Ok(second),
        ]));
        assert_eq!(
            call_proxy_with_body(chunked_state, "chunked-over-limit", chunked)
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[tokio::test]
    async fn cancellation_before_upstream_acceptance_releases_reservation_and_records_499() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("delayed upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    "data: [DONE]\n\n",
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("delayed upstream should serve");
        });
        let state = test_state(port, 1.0);
        let task_state = state.clone();
        let proxy_task = tokio::spawn(async move { call_proxy(task_state, "cancel-before").await });
        for _ in 0..100 {
            if state
                .budget_ledger
                .user_snapshot("cancel-before")
                .reserved_spend
                > 0.0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            state
                .budget_ledger
                .user_snapshot("cancel-before")
                .reserved_spend
                > 0.0
        );
        proxy_task.abort();
        let _ = proxy_task.await;

        let snapshot = state.budget_ledger.user_snapshot("cancel-before");
        assert_eq!(snapshot.reserved_spend, 0.0);
        assert_eq!(snapshot.committed_spend, 0.0);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(499)
        );
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_after_acceptance_keeps_prompt_and_records_499_once() {
        let (port, server) = spawn_static_upstream(
            StatusCode::OK,
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        )
        .await;
        let state = test_state(port, 1.0);
        let response = call_proxy(state.clone(), "cancel-after").await;
        drop(response);

        let snapshot = state.budget_ledger.user_snapshot("cancel-after");
        assert!(snapshot.committed_spend > 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
        let requests = state.recent_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests.front().map(|record| record.status), Some(499));
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_during_stream_before_next_budget_cutoff_charges_only_parsed_output() {
        const FIRST_EVENT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        const SECOND_EVENT: &str = "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("streaming upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(|| async {
                let body = Body::from_stream(stream::unfold(0, |index| async move {
                    let event = match index {
                        0 => FIRST_EVENT,
                        1 => SECOND_EVENT,
                        _ => return None,
                    };
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Some((
                        Ok::<Bytes, std::io::Error>(Bytes::from_static(event.as_bytes())),
                        index + 1,
                    ))
                }));
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                    .body(body)
                    .unwrap()
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("streaming upstream should serve");
        });
        let pricing = get_model_pricing("gpt-4o-mini");
        let first_tokens = bpe_for_model("gpt-4o-mini")
            .unwrap()
            .encode_with_special_tokens("hello")
            .len();
        let limit = proxy_prompt_cost() + first_tokens as f64 * pricing.output_cost_per_token;
        let state = test_state_with_budgets(port, 1.0, limit);
        let response = call_proxy(state.clone(), "cancel-midstream").await;
        let mut body = response.into_body();
        let first = body
            .frame()
            .await
            .expect("first frame should arrive")
            .expect("first frame should be valid")
            .into_data()
            .expect("first frame should contain data");
        assert_eq!(first, Bytes::from_static(FIRST_EVENT.as_bytes()));
        drop(body);

        let snapshot = state.budget_ledger.user_snapshot("cancel-midstream");
        assert_eq!(snapshot.total_spend, limit);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(499)
        );
        server.abort();
    }

    #[tokio::test]
    async fn upstream_disconnect_during_stream_records_failure_without_refunding_prompt() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("disconnecting upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener
                .accept()
                .await
                .expect("proxy connection should arrive");
            let mut request = vec![0_u8; 4096];
            let _ = socket.read(&mut request).await;
            let event = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
            let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            socket.write_all(headers).await.unwrap();
            socket
                .write_all(format!("{:x}\r\n", event.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(event).await.unwrap();
            socket.write_all(b"\r\n10\r\npartial").await.unwrap();
        });
        let state = test_state(port, 1.0);
        let response = call_proxy(state.clone(), "disconnect-stream").await;
        assert!(response.into_body().collect().await.is_err());
        let snapshot = state.budget_ledger.user_snapshot("disconnect-stream");
        assert!(snapshot.committed_spend > 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(502)
        );
        server.abort();
    }

    #[tokio::test]
    async fn upstream_disconnect_before_headers_releases_reservation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("raw upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("connection should arrive");
            drop(socket);
        });
        let state = test_state(port, 1.0);
        let response = call_proxy(state.clone(), "disconnect-headers").await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("disconnect-headers")
                .total_spend,
            0.0
        );
        server.await.expect("raw upstream task should finish");
    }

    #[tokio::test]
    async fn upstream_dns_failure_releases_reservation() {
        let mut state = test_state(0, 1.0);
        state.openai_upstream_url =
            "http://kilovolt-dns-failure.invalid/v1/chat/completions".to_string();
        state.client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("DNS failure test HTTP client should build");

        let mut headers = proxy_headers("dns-failure");
        headers.remove("x-mock-upstream");
        let response = chat_completions_proxy(State(state.clone()), headers, proxy_body()).await;

        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_GATEWAY | StatusCode::GATEWAY_TIMEOUT
            ),
            "DNS resolution must either fail or hit the response-header timeout"
        );
        assert_eq!(
            state.budget_ledger.user_snapshot("dns-failure").total_spend,
            0.0
        );
        assert!(matches!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(502 | 504)
        ));
    }

    #[tokio::test]
    async fn upstream_header_timeout_releases_reservation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("slow upstream should bind");
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    "data: [DONE]\n\n",
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("slow upstream should serve");
        });
        let state = test_state(port, 1.0);
        let response = call_proxy(state.clone(), "timeout-user").await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("timeout-user")
                .total_spend,
            0.0
        );
        server.abort();
    }
}
