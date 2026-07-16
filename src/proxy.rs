use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::stream::{self, StreamExt};
use std::pin::Pin;
use std::time::Instant;
use std::sync::atomic::Ordering;
use tiktoken_rs::{num_tokens_from_messages, ChatCompletionRequestMessage};
use tracing::{error, info, warn};

use crate::config::AppState;
use crate::budget::{get_model_pricing, StreamMonitor};

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

/// A mock upstream chat completion route that returns a standard chunked SSE event stream
/// with an artificial delay between events. Useful for verifying reverse-proxy streaming and termination.
pub async fn mock_chat_completions() -> Response {
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

type BoxedByteStream = Pin<Box<dyn futures_util::stream::Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

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
                    state.record_request(&request_id, "anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
                state.record_request(&request_id, "anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
            state.record_request(&request_id, "anonymous", "unknown", 401, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
                    state.record_request(&request_id, "anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
                state.record_request(&request_id, "anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
            state.record_request(&request_id, "anonymous", "unknown", 400, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
            state.record_request(&request_id, &user_id, "unknown", 400, start_time.elapsed().as_millis() as u64, 0, 0.0);
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
            state.record_request(&request_id, &user_id, "unknown", 400, start_time.elapsed().as_millis() as u64, 0, 0.0);
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Invalid JSON payload",
                "invalid_request_error",
                None,
            );
        }
    };

    let is_gemini = request.model.starts_with("gemini-");

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
    let pipeline_id = headers.get("X-Pipeline-ID")
        .and_then(|val| val.to_str().ok())
        .map(|s| s.to_string());
    let pipeline_name = headers.get("X-Pipeline-Name")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("unknown-pipeline");
    let step_name = headers.get("X-Step-Name")
        .and_then(|val| val.to_str().ok())
        .unwrap_or("unknown-step");

    if let Err(err_msg) = crate::budget::check_token_budgets(&state, pipeline_id.as_deref(), prompt_tokens) {
        warn!(
            pipeline = %pipeline_name,
            step = %step_name,
            error = %err_msg,
            "[pipeline:{}][step:{}] {}", pipeline_name, step_name, err_msg
        );
        state.record_request(&request_id, &user_id, &request.model, 429, start_time.elapsed().as_millis() as u64, prompt_tokens, 0.0);
        return make_error_response(
            StatusCode::TOO_MANY_REQUESTS,
            &err_msg,
            "requests",
            Some("budget_exceeded"),
        );
    }

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
        state.record_request(&request_id, &user_id, &request.model, 429, start_time.elapsed().as_millis() as u64, prompt_tokens, prompt_cost);
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

    // Extract raw API Key for downstream delivery
    let api_key = auth_val.to_str().unwrap_or("").trim_start_matches("Bearer ").to_string();

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
        "https://api.openai.com/v1/chat/completions".to_string()
    };

    // 8. Prepare Upstream Request. We format payload according to provider targets.
    let mut upstream_req = state.client.post(&upstream_url);

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

        let gemini_contents: Vec<GeminiContent> = request.messages.iter().map(|msg| {
            let role = match msg.role.as_str() {
                "assistant" => "model",
                r => r,
            }.to_string();

            let content_str = match &msg.content {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(val) => val.to_string(),
                None => "".to_string(),
            };

            GeminiContent {
                role,
                parts: vec![GeminiPart { text: content_str }],
            }
        }).collect();

        let gemini_req = GeminiRequest { contents: gemini_contents };
        let gemini_body = serde_json::to_vec(&gemini_req).unwrap();
        upstream_req = upstream_req.body(gemini_body);
    } else {
        // Standard OpenAI layout
        upstream_req = upstream_req
            .header(reqwest::header::AUTHORIZATION, auth_val)
            .header(reqwest::header::CONTENT_TYPE, content_type_val)
            .body(body_bytes.clone());
    }

    info!("Initiating handshake with upstream provider at {}...", upstream_url);
    let upstream_res = match upstream_req.send().await {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to connect to upstream: {:?}", e);
            refund_prompt_cost();
            state.record_request(&request_id, &user_id, &request.model, 502, start_time.elapsed().as_millis() as u64, prompt_tokens, 0.0);
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
                state.record_request(&request_id, &user_id, &request.model, 502, start_time.elapsed().as_millis() as u64, prompt_tokens, 0.0);
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

        state.record_request(&request_id, &user_id, &request.model, status.as_u16(), start_time.elapsed().as_millis() as u64, prompt_tokens, 0.0);

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
        request_id,
        user_id,
        state.spend_tracker.clone(),
        request.model,
        pricing,
        prompt_tokens,
        prompt_cost,
        total_spend,
        state.default_budget,
        state.clone(), // Pass state to enable stats updates on close/cancel
        is_gemini,
        pipeline_id,
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
