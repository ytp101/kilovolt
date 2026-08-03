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

use crate::budget::StreamMonitor;
use crate::config::{AppState, secrets_match};
use crate::ledger::{BudgetError, BudgetScope, BudgetSnapshot};
use crate::pricing::Provider;

// Structs for incoming request body parsing
#[derive(serde::Deserialize, Clone)]
#[allow(dead_code)]
struct IncomingRequest {
    model: String,
    messages: Vec<IncomingMessage>,
    #[serde(default)]
    stream: bool,
    max_completion_tokens: Option<serde_json::Value>,
    max_tokens: Option<serde_json::Value>,
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

fn proxy_credentials_valid(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = state.proxy_token.as_deref() else {
        return true;
    };
    headers
        .get("x-kilovolt-key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| secrets_match(expected, actual))
}

fn proxy_auth_error() -> Response {
    make_error_response(
        StatusCode::UNAUTHORIZED,
        "Kilovolt proxy authentication failed",
        "authentication_error",
        Some("kilovolt_proxy_auth_failed"),
    )
}

fn evaluation_gateway_auth_error() -> Response {
    make_error_response(
        StatusCode::UNAUTHORIZED,
        "Kilovolt gateway key authentication failed",
        "authentication_error",
        Some("kilovolt_gateway_auth_failed"),
    )
}

fn evaluation_setup_required_error() -> Response {
    make_error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Complete local evaluation setup at http://127.0.0.1:8080 before proxying requests",
        "invalid_request_error",
        Some("setup_required"),
    )
}

/// Deterministic local mock upstream used by tests and the benchmark harness.
pub async fn mock_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !state.mock_upstream_enabled {
        return make_error_response(
            StatusCode::NOT_FOUND,
            "Kilovolt mock upstream is disabled",
            "invalid_request_error",
            Some("mock_upstream_disabled"),
        );
    }
    if !proxy_credentials_valid(&state, &headers) {
        return proxy_auth_error();
    }
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

#[derive(Clone, Copy, Debug)]
enum ReservationMode {
    Streaming,
    NonStreaming {
        maximum_output_tokens: usize,
        maximum_output_cost: f64,
        accepted: bool,
    },
}

struct RequestReservationGuard {
    state: AppState,
    request_id: String,
    user_id: String,
    model: String,
    prompt_tokens: usize,
    prompt_cost: f64,
    start_time: Instant,
    mode: ReservationMode,
    active: bool,
}

impl RequestReservationGuard {
    fn streaming(
        state: AppState,
        request_id: &str,
        user_id: &str,
        model: &str,
        prompt_tokens: usize,
        prompt_cost: f64,
        start_time: Instant,
    ) -> Self {
        Self {
            state,
            request_id: request_id.to_string(),
            user_id: user_id.to_string(),
            model: model.to_string(),
            prompt_tokens,
            prompt_cost,
            start_time,
            mode: ReservationMode::Streaming,
            active: true,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn non_streaming(
        state: AppState,
        request_id: &str,
        user_id: &str,
        model: &str,
        prompt_tokens: usize,
        prompt_cost: f64,
        maximum_output_tokens: usize,
        maximum_output_cost: f64,
        start_time: Instant,
    ) -> Self {
        Self {
            state,
            request_id: request_id.to_string(),
            user_id: user_id.to_string(),
            model: model.to_string(),
            prompt_tokens,
            prompt_cost,
            start_time,
            mode: ReservationMode::NonStreaming {
                maximum_output_tokens,
                maximum_output_cost,
                accepted: false,
            },
            active: true,
        }
    }

    fn commit_streaming(&mut self) -> Result<BudgetSnapshot, BudgetError> {
        if !matches!(self.mode, ReservationMode::Streaming) {
            return Err(BudgetError::InvalidReservationTransition);
        }
        let result = self
            .state
            .budget_ledger
            .commit_prompt(&self.request_id, &self.user_id);
        if result.is_ok() {
            self.active = false;
        }
        result
    }

    fn accept_non_stream(&mut self) -> Result<BudgetSnapshot, BudgetError> {
        let ReservationMode::NonStreaming { accepted, .. } = &mut self.mode else {
            return Err(BudgetError::InvalidReservationTransition);
        };
        // Successful upstream headers mean provider generation may already be
        // billable. Mark the guard before the ledger transition so cancellation
        // or an internal transition failure finalizes conservatively.
        *accepted = true;
        self.state
            .budget_ledger
            .accept_non_stream_prompt(&self.request_id, &self.user_id)
    }

    fn settle_non_stream(
        &mut self,
        actual_output_cost: f64,
    ) -> Result<BudgetSnapshot, BudgetError> {
        if !matches!(
            self.mode,
            ReservationMode::NonStreaming { accepted: true, .. }
        ) {
            return Err(BudgetError::InvalidReservationTransition);
        }
        let result = self.state.budget_ledger.settle_non_stream_output(
            &self.request_id,
            &self.user_id,
            actual_output_cost,
        );
        if result.is_ok() {
            self.active = false;
        }
        result
    }

    fn finalize_non_stream_conservatively(
        &mut self,
        reason: &str,
    ) -> Result<BudgetSnapshot, BudgetError> {
        if !matches!(
            self.mode,
            ReservationMode::NonStreaming { accepted: true, .. }
        ) {
            return Err(BudgetError::InvalidReservationTransition);
        }
        let result = self
            .state
            .budget_ledger
            .finalize_unknown_output_conservatively(&self.request_id, &self.user_id);
        match &result {
            Ok(snapshot) => info!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                reason = %reason,
                project_total_spend = %snapshot.project.total_spend,
                user_total_spend = %snapshot.user.total_spend,
                "Conservatively committed the full non-streaming output reservation"
            ),
            Err(error) => error!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                reason = %reason,
                error = %error,
                "Failed to conservatively finalize non-streaming output"
            ),
        }
        if result.is_ok() {
            if let Some((maximum_output_tokens, _)) = self.maximum_non_stream_output() {
                self.state
                    .total_tokens_consumed
                    .fetch_add(maximum_output_tokens, Ordering::Relaxed);
            }
            self.active = false;
        }
        result
    }

    fn maximum_non_stream_output(&self) -> Option<(usize, f64)> {
        match self.mode {
            ReservationMode::Streaming => None,
            ReservationMode::NonStreaming {
                maximum_output_tokens,
                maximum_output_cost,
                ..
            } => Some((maximum_output_tokens, maximum_output_cost)),
        }
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

impl Drop for RequestReservationGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let (tokens, cost, result) = match self.mode {
            ReservationMode::NonStreaming {
                maximum_output_tokens,
                maximum_output_cost,
                accepted: true,
            } => (
                self.prompt_tokens.saturating_add(maximum_output_tokens),
                self.prompt_cost + maximum_output_cost,
                self.state
                    .budget_ledger
                    .finalize_unknown_output_conservatively(&self.request_id, &self.user_id),
            ),
            ReservationMode::Streaming
            | ReservationMode::NonStreaming {
                accepted: false, ..
            } => (
                self.prompt_tokens,
                0.0,
                self.state
                    .budget_ledger
                    .release_prompt(&self.request_id, &self.user_id),
            ),
        };
        if let Err(error) = result {
            error!(
                request_id = %self.request_id,
                user_id = %self.user_id,
                error = %error,
                "Failed to finalize reservation after request task cancellation"
            );
        } else if matches!(
            self.mode,
            ReservationMode::NonStreaming { accepted: true, .. }
        ) {
            self.state
                .total_tokens_consumed
                .fetch_add(tokens.saturating_sub(self.prompt_tokens), Ordering::Relaxed);
        }
        self.state.record_request(
            &self.request_id,
            &self.user_id,
            &self.model,
            499,
            self.start_time.elapsed().as_millis() as u64,
            tokens,
            cost,
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

fn token_count(model: &str, text: &str) -> usize {
    bpe_for_model(model)
        .ok()
        .or_else(|| bpe_for_model("gpt-4o").ok())
        .map_or_else(
            || text.len().div_ceil(4),
            |bpe| bpe.encode_with_special_tokens(text).len(),
        )
}

fn checked_token_cost(tokens: usize, per_token: f64) -> Result<f64, &'static str> {
    let cost = tokens as f64 * per_token;
    if cost.is_finite() && cost >= 0.0 {
        Ok(cost)
    } else {
        Err("token cost overflowed")
    }
}

fn parse_positive_token_bound(
    value: &serde_json::Value,
    field: &'static str,
) -> Result<usize, &'static str> {
    let raw = value
        .as_u64()
        .ok_or("non-stream output token bound must be a positive integer")?;
    if raw == 0 {
        return Err("non-stream output token bound must be a positive integer");
    }
    usize::try_from(raw).map_err(|_| field)
}

fn select_non_stream_output_bound(
    request: &IncomingRequest,
    configured_default: Option<usize>,
) -> Result<usize, &'static str> {
    if let Some(value) = request.max_completion_tokens.as_ref() {
        return parse_positive_token_bound(value, "max_completion_tokens overflowed");
    }
    if let Some(value) = request.max_tokens.as_ref() {
        return parse_positive_token_bound(value, "max_tokens overflowed");
    }
    configured_default.ok_or("non-streaming requests require a maximum output token bound")
}

fn is_nonempty_json(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(_) => true,
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(values) => values.iter().any(is_nonempty_json),
        serde_json::Value::Object(values) => values.values().any(is_nonempty_json),
    }
}

fn prompt_requires_canonical_accounting(request: &serde_json::Value) -> bool {
    const ADVANCED_TOP_LEVEL: &[&str] = &[
        "tools",
        "functions",
        "tool_choice",
        "function_call",
        "response_format",
    ];
    if ADVANCED_TOP_LEVEL
        .iter()
        .any(|field| request.get(*field).is_some_and(is_nonempty_json))
    {
        return true;
    }
    request
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .is_some_and(|content| content.is_array())
                    || ["function_call", "tool_calls", "tool_call_id", "refusal"]
                        .iter()
                        .any(|field| message.get(*field).is_some_and(is_nonempty_json))
            })
        })
}

fn canonical_prompt_tokens(model: &str, request: &serde_json::Value, basic_tokens: usize) -> usize {
    if !prompt_requires_canonical_accounting(request) {
        return basic_tokens;
    }
    let mut canonical = serde_json::Map::new();
    for field in [
        "messages",
        "tools",
        "functions",
        "tool_choice",
        "function_call",
        "response_format",
    ] {
        if let Some(value) = request.get(field)
            && is_nonempty_json(value)
        {
            canonical.insert(field.to_string(), value.clone());
        }
    }
    let serialized = serde_json::Value::Object(canonical).to_string();
    basic_tokens.max(token_count(model, &serialized))
}

#[derive(Debug)]
struct GeneratedRepresentation {
    canonical: serde_json::Value,
    plain_text: Option<String>,
}

fn canonicalize_structured_content(
    content: &serde_json::Value,
) -> Result<serde_json::Value, &'static str> {
    let parts = content
        .as_array()
        .ok_or("structured completion content was not an array")?;
    let mut canonical_parts = Vec::with_capacity(parts.len());
    for part in parts {
        let object = part
            .as_object()
            .ok_or("structured completion part was not an object")?;
        for (field, value) in object {
            if !matches!(field.as_str(), "type" | "text" | "refusal") && is_nonempty_json(value) {
                return Err("unsupported billable structured content field");
            }
        }
        let part_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or("structured completion part had no supported type")?;
        if !matches!(part_type, "text" | "refusal") {
            return Err("unsupported billable structured content type");
        }
        let mut canonical = serde_json::Map::new();
        canonical.insert(
            "type".to_string(),
            serde_json::Value::String(part_type.to_string()),
        );
        let value_field = if part_type == "text" {
            "text"
        } else {
            "refusal"
        };
        let text = object
            .get(value_field)
            .and_then(serde_json::Value::as_str)
            .ok_or("structured completion part had no supported text")?;
        canonical.insert(
            value_field.to_string(),
            serde_json::Value::String(text.to_string()),
        );
        canonical_parts.push(serde_json::Value::Object(canonical));
    }
    Ok(serde_json::Value::Array(canonical_parts))
}

fn canonicalize_function_call(
    value: &serde_json::Value,
) -> Result<serde_json::Value, &'static str> {
    let object = value.as_object().ok_or("function_call was not an object")?;
    for (field, value) in object {
        if !matches!(field.as_str(), "name" | "arguments") && is_nonempty_json(value) {
            return Err("unsupported billable function_call field");
        }
    }
    let mut canonical = serde_json::Map::new();
    for field in ["name", "arguments"] {
        if let Some(value) = object.get(field)
            && !value.is_null()
        {
            let string = value
                .as_str()
                .ok_or("function_call field was not a string")?;
            canonical.insert(
                field.to_string(),
                serde_json::Value::String(string.to_string()),
            );
        }
    }
    Ok(serde_json::Value::Object(canonical))
}

fn canonicalize_tool_calls(value: &serde_json::Value) -> Result<serde_json::Value, &'static str> {
    let calls = value.as_array().ok_or("tool_calls was not an array")?;
    let mut canonical_calls = Vec::with_capacity(calls.len());
    for call in calls {
        let object = call.as_object().ok_or("tool call was not an object")?;
        for (field, value) in object {
            if !matches!(field.as_str(), "index" | "id" | "type" | "function")
                && is_nonempty_json(value)
            {
                return Err("unsupported billable tool-call field");
            }
        }
        let mut canonical = serde_json::Map::new();
        for field in ["id", "type"] {
            if let Some(value) = object.get(field)
                && !value.is_null()
            {
                let string = value
                    .as_str()
                    .ok_or("tool-call identifier or type was not a string")?;
                canonical.insert(
                    field.to_string(),
                    serde_json::Value::String(string.to_string()),
                );
            }
        }
        if let Some(function) = object.get("function")
            && !function.is_null()
        {
            canonical.insert(
                "function".to_string(),
                canonicalize_function_call(function)?,
            );
        }
        canonical_calls.push(serde_json::Value::Object(canonical));
    }
    Ok(serde_json::Value::Array(canonical_calls))
}

fn canonicalize_generated_object(
    value: &serde_json::Value,
) -> Result<GeneratedRepresentation, &'static str> {
    let object = value
        .as_object()
        .ok_or("generated completion payload was not an object")?;
    for (field, value) in object {
        if !matches!(
            field.as_str(),
            "role" | "content" | "refusal" | "function_call" | "tool_calls"
        ) && is_nonempty_json(value)
        {
            return Err("unsupported billable output field");
        }
    }

    let mut canonical = serde_json::Map::new();
    let mut plain_text = Some(String::new());
    let mut advanced = false;
    if let Some(content) = object.get("content")
        && !content.is_null()
    {
        match content {
            serde_json::Value::String(text) => {
                plain_text = Some(text.clone());
                canonical.insert("content".to_string(), content.clone());
            }
            serde_json::Value::Array(_) => {
                advanced = true;
                canonical.insert(
                    "content".to_string(),
                    canonicalize_structured_content(content)?,
                );
            }
            _ => return Err("completion content had an unsupported type"),
        }
    }
    if let Some(refusal) = object.get("refusal")
        && !refusal.is_null()
    {
        advanced = true;
        let refusal = refusal
            .as_str()
            .ok_or("completion refusal was not a string")?;
        canonical.insert(
            "refusal".to_string(),
            serde_json::Value::String(refusal.to_string()),
        );
    }
    if let Some(function_call) = object.get("function_call")
        && !function_call.is_null()
    {
        advanced = true;
        canonical.insert(
            "function_call".to_string(),
            canonicalize_function_call(function_call)?,
        );
    }
    if let Some(tool_calls) = object.get("tool_calls")
        && !tool_calls.is_null()
    {
        advanced = true;
        canonical.insert(
            "tool_calls".to_string(),
            canonicalize_tool_calls(tool_calls)?,
        );
    }

    Ok(GeneratedRepresentation {
        canonical: serde_json::Value::Object(canonical),
        plain_text: if advanced { None } else { plain_text },
    })
}

fn locally_accounted_choice_tokens(
    model: &str,
    choices: &[serde_json::Value],
    payload_field: &str,
) -> Result<usize, &'static str> {
    let mut total_tokens = 0usize;
    for choice in choices {
        let object = choice.as_object().ok_or("choice was not an object")?;
        for (field, value) in object {
            if !matches!(
                field.as_str(),
                "index" | "message" | "delta" | "finish_reason" | "logprobs"
            ) && is_nonempty_json(value)
            {
                return Err("unsupported billable output field");
            }
        }
        let generated = object
            .get(payload_field)
            .ok_or("choice had no supported generated payload")?;
        let representation = canonicalize_generated_object(generated)?;
        let choice_tokens = if let Some(text) = representation.plain_text {
            token_count(model, &text)
        } else {
            token_count(model, &representation.canonical.to_string())
        };
        total_tokens = checked_add_completion_tokens(total_tokens, choice_tokens)?;
    }
    Ok(total_tokens)
}

fn checked_add_completion_tokens(
    accumulated: usize,
    choice_tokens: usize,
) -> Result<usize, &'static str> {
    accumulated
        .checked_add(choice_tokens)
        .ok_or("completion token count overflowed")
}

pub(crate) fn streaming_output_tokens(
    model: &str,
    value: &serde_json::Value,
) -> Result<usize, &'static str> {
    let choices = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .ok_or("streaming frame had no choices array")?;
    locally_accounted_choice_tokens(model, choices, "delta")
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

    let choices = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .ok_or("response had neither supported usage nor choices")?;
    let tokens = locally_accounted_choice_tokens(model, choices, "message")?;
    Ok((tokens, "local supported-message tokenizer fallback"))
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

fn conservatively_finalize_non_stream_failure(
    guard: &mut RequestReservationGuard,
    state: &AppState,
    pipeline_id: Option<&str>,
    reason: &str,
) -> (usize, f64) {
    let (maximum_output_tokens, maximum_output_cost) = guard
        .maximum_non_stream_output()
        .expect("non-stream failure helper requires a non-stream reservation");
    let total_tokens = guard.prompt_tokens.saturating_add(maximum_output_tokens);
    let total_cost = guard.prompt_cost + maximum_output_cost;
    let _ = guard.finalize_non_stream_conservatively(reason);
    record_token_budget_usage(state, pipeline_id, total_tokens);
    (total_tokens, total_cost)
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

    let evaluation_setup = if state.evaluation_mode() {
        match state.evaluation_setup_snapshot() {
            Some(setup) => Some(setup),
            None => return evaluation_setup_required_error(),
        }
    } else {
        None
    };

    if evaluation_setup.is_none() && !proxy_credentials_valid(&state, &headers) {
        state.record_request(
            &request_id,
            "anonymous",
            "unknown",
            401,
            start_time.elapsed().as_millis() as u64,
            0,
            0.0,
        );
        return proxy_auth_error();
    }
    if headers.contains_key("x-mock-upstream") && !state.mock_upstream_enabled {
        state.record_request(
            &request_id,
            "anonymous",
            "unknown",
            403,
            start_time.elapsed().as_millis() as u64,
            0,
            0.0,
        );
        return make_error_response(
            StatusCode::FORBIDDEN,
            "X-Mock-Upstream requires KILOVOLT_ENABLE_MOCK_UPSTREAM=true",
            "invalid_request_error",
            Some("mock_upstream_disabled"),
        );
    }

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

    let upstream_auth_val = if let Some(setup) = &evaluation_setup {
        let gateway_key_is_valid = auth_val
            .to_str()
            .ok()
            .and_then(|authorization| authorization.strip_prefix("Bearer "))
            .is_some_and(|gateway_key| secrets_match(setup.gateway_key(), gateway_key));
        if !gateway_key_is_valid {
            state.record_request(
                &request_id,
                "anonymous",
                "unknown",
                401,
                start_time.elapsed().as_millis() as u64,
                0,
                0.0,
            );
            return evaluation_gateway_auth_error();
        }

        axum::http::HeaderValue::from_str(&format!("Bearer {}", setup.provider_api_key()))
            .expect("evaluation provider key was validated during setup")
    } else {
        auth_val.clone()
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
    let mut request_json: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
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
    let request: IncomingRequest = match serde_json::from_value(request_json.clone()) {
        Ok(request) => request,
        Err(error) => {
            warn!("Request JSON did not match the supported shape: {error}");
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
                "Request JSON did not match the supported chat-completions shape",
                "invalid_request_error",
                Some("invalid_request_shape"),
            );
        }
    };

    let is_gemini = request.model.starts_with("gemini-");
    if evaluation_setup.is_some() && is_gemini {
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
            "Browser evaluation mode supports OpenAI models only",
            "invalid_request_error",
            Some("evaluation_openai_only"),
        );
    }
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

    let provider = Provider::for_model(&request.model);
    let resolved_pricing = match state.pricing_registry.resolve(provider, &request.model) {
        Ok(pricing) => pricing,
        Err(error) => {
            warn!(
                model = %request.model,
                provider = %provider,
                error = %error,
                "Rejecting request because model pricing is not configured"
            );
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
                "Model pricing is not configured for this provider/model",
                "invalid_request_error",
                Some("model_pricing_not_configured"),
            );
        }
    };
    let pricing = resolved_pricing.pricing;
    info!(
        model = %request.model,
        provider = %provider,
        pricing_source = %resolved_pricing.source,
        pricing_match = %resolved_pricing.match_type,
        pricing_pattern = %resolved_pricing.matched_model,
        effective_date = ?resolved_pricing.effective_date,
        "Resolved request pricing"
    );

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

    let basic_prompt_tokens = match num_tokens_from_messages(&request.model, &tiktoken_messages) {
        Ok(t) => t,
        Err(_) => {
            // Fallback to standard gpt-4o tokenization
            num_tokens_from_messages("gpt-4o", &tiktoken_messages).unwrap_or(0)
        }
    };
    let prompt_tokens = canonical_prompt_tokens(&request.model, &request_json, basic_prompt_tokens);

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

    let prompt_cost = match checked_token_cost(prompt_tokens, pricing.input_cost_per_token) {
        Ok(cost) => cost,
        Err(reason) => {
            error!(model = %request.model, prompt_tokens, reason, "Prompt cost overflow");
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Prompt cost could not be represented safely",
                "invalid_request_error",
                Some("accounting_overflow"),
            );
        }
    };

    let maximum_non_stream_output = if request.stream {
        None
    } else {
        let maximum_tokens = match select_non_stream_output_bound(
            &request,
            state.non_stream_default_max_output_tokens,
        ) {
            Ok(tokens) => tokens,
            Err(reason) => {
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    400,
                    start_time.elapsed().as_millis() as u64,
                    prompt_tokens,
                    0.0,
                );
                return make_error_response(
                    StatusCode::BAD_REQUEST,
                    reason,
                    "invalid_request_error",
                    Some("non_stream_output_bound_required"),
                );
            }
        };
        if prompt_tokens.checked_add(maximum_tokens).is_none() {
            return make_error_response(
                StatusCode::BAD_REQUEST,
                "Prompt plus maximum output tokens overflowed internal accounting",
                "invalid_request_error",
                Some("accounting_overflow"),
            );
        }
        let maximum_cost = match checked_token_cost(maximum_tokens, pricing.output_cost_per_token) {
            Ok(cost) => cost,
            Err(reason) => {
                error!(
                    model = %request.model,
                    maximum_tokens,
                    reason,
                    "Maximum output reservation overflow"
                );
                return make_error_response(
                    StatusCode::BAD_REQUEST,
                    "Maximum output cost could not be represented safely",
                    "invalid_request_error",
                    Some("accounting_overflow"),
                );
            }
        };
        if request.max_completion_tokens.is_none() && request.max_tokens.is_none() {
            let Some(object) = request_json.as_object_mut() else {
                return make_error_response(
                    StatusCode::BAD_REQUEST,
                    "Request body must be a JSON object",
                    "invalid_request_error",
                    Some("invalid_request_shape"),
                );
            };
            object.insert(
                "max_completion_tokens".to_string(),
                serde_json::json!(maximum_tokens),
            );
        }
        Some((maximum_tokens, maximum_cost))
    };

    let (project_budget_limit, user_budget_limit) = state.effective_budgets();
    let reservation_result = if let Some((_, maximum_output_cost)) = maximum_non_stream_output {
        state.budget_ledger.reserve_non_stream_request(
            &request_id,
            &user_id,
            prompt_cost,
            maximum_output_cost,
            project_budget_limit,
            user_budget_limit,
        )
    } else {
        state.budget_ledger.reserve_prompt(
            &request_id,
            &user_id,
            prompt_cost,
            project_budget_limit,
            user_budget_limit,
        )
    };
    let reservation = match reservation_result {
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
                project_budget_limit = %project_budget_limit,
                user_budget_limit = %user_budget_limit,
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
        maximum_output_tokens = ?maximum_non_stream_output.map(|(tokens, _)| tokens),
        maximum_output_cost = ?maximum_non_stream_output.map(|(_, cost)| cost),
        project_total_spend_with_reservations = %reservation.project.total_spend,
        user_total_spend_with_reservations = %reservation.user.total_spend,
        "Bankruptcy Shield: Reserved request cost"
    );
    let mut reservation_guard =
        if let Some((maximum_output_tokens, maximum_output_cost)) = maximum_non_stream_output {
            RequestReservationGuard::non_streaming(
                state.clone(),
                &request_id,
                &user_id,
                &request.model,
                prompt_tokens,
                prompt_cost,
                maximum_output_tokens,
                maximum_output_cost,
                start_time,
            )
        } else {
            RequestReservationGuard::streaming(
                state.clone(),
                &request_id,
                &user_id,
                &request.model,
                prompt_tokens,
                prompt_cost,
                start_time,
            )
        };

    // Update total tokens consumed globally after the financial reservation succeeds.
    state
        .total_tokens_consumed
        .fetch_add(prompt_tokens, Ordering::Relaxed);

    // Extract raw API Key for downstream delivery
    let api_key = upstream_auth_val
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
        if let Some(proxy_token) = state.proxy_token.as_deref() {
            upstream_req = upstream_req.header("x-kilovolt-key", proxy_token);
        }
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
        let forwarded_body = match serde_json::to_vec(&request_json) {
            Ok(body) => body,
            Err(error) => {
                reservation_guard.release("OpenAI request serialization failed");
                error!("Failed to serialize forwarded request: {error}");
                return make_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to prepare upstream request",
                    "api_error",
                    None,
                );
            }
        };
        upstream_req = upstream_req
            .header(reqwest::header::AUTHORIZATION, upstream_auth_val)
            .header(reqwest::header::CONTENT_TYPE, content_type_val)
            .body(forwarded_body);
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

    let committed = match if request.stream {
        reservation_guard.commit_streaming()
    } else {
        reservation_guard.accept_non_stream()
    } {
        Ok(snapshot) => snapshot,
        Err(error) => {
            error!(
                user_id = %user_id,
                request_id = %request_id,
                error = %error,
                "Failed to commit prompt reservation after upstream acceptance"
            );
            let (recorded_tokens, recorded_cost) = if request.stream {
                (prompt_tokens, 0.0)
            } else {
                conservatively_finalize_non_stream_failure(
                    &mut reservation_guard,
                    &state,
                    pipeline_id.as_deref(),
                    "ledger acceptance transition failed after provider acceptance",
                )
            };
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                500,
                start_time.elapsed().as_millis() as u64,
                recorded_tokens,
                recorded_cost,
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
        non_stream_output_reserved = %(!request.stream),
        "Upstream request succeeded. Committed prompt cost; any non-streaming output maximum remains reserved."
    );

    let upstream_headers = upstream_res.headers().clone();

    if !request.stream {
        let (maximum_output_tokens, maximum_output_cost) = reservation_guard
            .maximum_non_stream_output()
            .expect("non-stream request must have an output reservation");
        if !content_type_starts_with(&upstream_headers, "application/json") {
            let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                &mut reservation_guard,
                &state,
                pipeline_id.as_deref(),
                "invalid upstream content type after acceptance",
            );
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                502,
                start_time.elapsed().as_millis() as u64,
                recorded_tokens,
                recorded_cost,
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
                    let (recorded_tokens, recorded_cost) =
                        conservatively_finalize_non_stream_failure(
                            &mut reservation_guard,
                            &state,
                            pipeline_id.as_deref(),
                            "upstream body exceeded limit after acceptance",
                        );
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        recorded_tokens,
                        recorded_cost,
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
                    let (recorded_tokens, recorded_cost) =
                        conservatively_finalize_non_stream_failure(
                            &mut reservation_guard,
                            &state,
                            pipeline_id.as_deref(),
                            "upstream disconnected after acceptance",
                        );
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        recorded_tokens,
                        recorded_cost,
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
                let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                    &mut reservation_guard,
                    &state,
                    pipeline_id.as_deref(),
                    "malformed JSON after acceptance",
                );
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    502,
                    start_time.elapsed().as_millis() as u64,
                    recorded_tokens,
                    recorded_cost,
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
                    let (recorded_tokens, recorded_cost) =
                        conservatively_finalize_non_stream_failure(
                            &mut reservation_guard,
                            &state,
                            pipeline_id.as_deref(),
                            reason,
                        );
                    state.record_request(
                        &request_id,
                        &user_id,
                        &request.model,
                        502,
                        start_time.elapsed().as_millis() as u64,
                        recorded_tokens,
                        recorded_cost,
                    );
                    return make_error_response(
                        StatusCode::BAD_GATEWAY,
                        &format!("Unsupported non-streaming upstream response: {reason}"),
                        "api_error",
                        Some("unsupported_billable_output_field"),
                    );
                }
            };
        if output_tokens > maximum_output_tokens {
            warn!(
                model = %request.model,
                requested_output_token_bound = %maximum_output_tokens,
                reported_or_estimated_output_tokens = %output_tokens,
                reserved_output_cost = %maximum_output_cost,
                "Provider usage exceeded the reserved non-streaming output bound"
            );
            let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                &mut reservation_guard,
                &state,
                pipeline_id.as_deref(),
                "provider usage exceeded reserved output bound",
            );
            state.record_request(
                &request_id,
                &user_id,
                &request.model,
                502,
                start_time.elapsed().as_millis() as u64,
                recorded_tokens,
                recorded_cost,
            );
            return make_error_response(
                StatusCode::BAD_GATEWAY,
                "Provider usage exceeded the reserved maximum output bound",
                "api_error",
                Some("provider_usage_exceeded_reserved_bound"),
            );
        }
        let output_cost = match checked_token_cost(output_tokens, pricing.output_cost_per_token) {
            Ok(cost) => cost,
            Err(reason) => {
                let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                    &mut reservation_guard,
                    &state,
                    pipeline_id.as_deref(),
                    reason,
                );
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    502,
                    start_time.elapsed().as_millis() as u64,
                    recorded_tokens,
                    recorded_cost,
                );
                return make_error_response(
                    StatusCode::BAD_GATEWAY,
                    "Provider output cost could not be represented safely",
                    "api_error",
                    Some("accounting_overflow"),
                );
            }
        };
        let charged = match reservation_guard.settle_non_stream(output_cost) {
            Ok(snapshot) => snapshot,
            Err(BudgetError::OutputExceedsReservation) => {
                warn!(
                    user_id = %user_id,
                    output_tokens = %output_tokens,
                    output_cost = %output_cost,
                    maximum_output_tokens = %maximum_output_tokens,
                    maximum_output_cost = %maximum_output_cost,
                    "Calculated output exceeded its reservation"
                );
                let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                    &mut reservation_guard,
                    &state,
                    pipeline_id.as_deref(),
                    "calculated output exceeded reserved cost",
                );
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    502,
                    start_time.elapsed().as_millis() as u64,
                    recorded_tokens,
                    recorded_cost,
                );
                return make_error_response(
                    StatusCode::BAD_GATEWAY,
                    "Provider output exceeded its reserved accounting bound",
                    "api_error",
                    Some("provider_usage_exceeded_reserved_bound"),
                );
            }
            Err(error) => {
                error!(
                    request_id = %request_id,
                    user_id = %user_id,
                    error = %error,
                    "Failed to settle non-streaming output reservation"
                );
                let (recorded_tokens, recorded_cost) = conservatively_finalize_non_stream_failure(
                    &mut reservation_guard,
                    &state,
                    pipeline_id.as_deref(),
                    "internal settlement failure",
                );
                state.record_request(
                    &request_id,
                    &user_id,
                    &request.model,
                    500,
                    start_time.elapsed().as_millis() as u64,
                    recorded_tokens,
                    recorded_cost,
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
        user_budget_limit,
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
    use super::{
        IncomingRequest, canonical_prompt_tokens, canonicalize_generated_object,
        chat_completions_proxy, checked_add_completion_tokens, locally_accounted_choice_tokens,
        mock_chat_completions, non_stream_output_tokens, select_non_stream_output_bound,
        streaming_output_tokens, token_count,
    };
    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::routing::post;
    use futures_util::stream;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tiktoken_rs::{ChatCompletionRequestMessage, bpe_for_model, num_tokens_from_messages};

    use crate::config::{AppState, test_evaluation_state, test_state, test_state_with_budgets};
    use crate::pricing::{ModelPricing, PricingRegistry, Provider};

    fn get_model_pricing(model: &str) -> ModelPricing {
        PricingRegistry::built_in()
            .resolve(Provider::for_model(model), model)
            .expect("test model pricing should resolve")
            .pricing
    }

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

    fn find_choice_boundary_case(model: &str) -> (&'static str, &'static str) {
        const CANDIDATES: &[&str] = &["a", "b", "hello", " world", "{", "}", "1", "2"];
        for &left in CANDIDATES {
            for &right in CANDIDATES {
                let independent = token_count(model, left) + token_count(model, right);
                let concatenated = token_count(model, &format!("{left}{right}"));
                if independent > concatenated {
                    return (left, right);
                }
            }
        }
        panic!("test candidates did not expose a tokenizer boundary case");
    }

    fn independently_accounted_generated_tokens(
        model: &str,
        generated: &serde_json::Value,
    ) -> usize {
        let representation = canonicalize_generated_object(generated)
            .expect("generated payload should be supported");
        match representation.plain_text {
            Some(text) => token_count(model, &text),
            None => token_count(model, &representation.canonical.to_string()),
        }
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

    #[test]
    fn non_stream_output_bound_precedence_is_strict_and_positive() {
        let request: IncomingRequest = serde_json::from_value(serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [],
            "max_completion_tokens": 7,
            "max_tokens": 9
        }))
        .unwrap();
        assert_eq!(select_non_stream_output_bound(&request, Some(11)), Ok(7));

        let legacy: IncomingRequest = serde_json::from_value(serde_json::json!({
            "model": "gpt-4o-mini", "messages": [], "max_tokens": 9
        }))
        .unwrap();
        assert_eq!(select_non_stream_output_bound(&legacy, Some(11)), Ok(9));

        let configured: IncomingRequest = serde_json::from_value(serde_json::json!({
            "model": "gpt-4o-mini", "messages": []
        }))
        .unwrap();
        assert_eq!(
            select_non_stream_output_bound(&configured, Some(11)),
            Ok(11)
        );
        assert!(select_non_stream_output_bound(&configured, None).is_err());

        let zero: IncomingRequest = serde_json::from_value(serde_json::json!({
            "model": "gpt-4o-mini", "messages": [], "max_completion_tokens": 0
        }))
        .unwrap();
        assert!(select_non_stream_output_bound(&zero, Some(11)).is_err());
    }

    #[test]
    fn advanced_prompts_include_tools_calls_results_and_structured_content() {
        let basic = serde_json::json!({
            "messages": [{"role": "user", "content": "weather"}]
        });
        let tools = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "weather"}]},
                {"role": "assistant", "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": {"name": "forecast", "arguments": "{\"city\":\"Bangkok\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "sunny"}
            ],
            "tools": [{
                "type": "function",
                "function": {"name": "forecast", "parameters": {
                    "type": "object", "properties": {"city": {"type": "string"}}
                }}
            }],
            "response_format": {"type": "json_object"}
        });
        let basic_tokens = canonical_prompt_tokens("gpt-4o-mini", &basic, 10);
        let advanced_tokens = canonical_prompt_tokens("gpt-4o-mini", &tools, 10);
        assert_eq!(basic_tokens, 10);
        assert!(advanced_tokens > basic_tokens);
    }

    #[test]
    fn supported_generated_tool_and_function_fields_are_accounted() {
        let function_frame = serde_json::json!({
            "choices": [{"delta": {"function_call": {
                "name": "forecast", "arguments": "{\"city\":\"Bangkok\"}"
            }}}]
        });
        let tool_frame = serde_json::json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "type": "function",
                 "function": {"name": "forecast", "arguments": "{\"city\":"}},
                {"index": 1, "id": "call_2", "type": "function",
                 "function": {"name": "clock", "arguments": "{\"zone\":\"UTC\"}"}}
            ]}}]
        });
        assert!(streaming_output_tokens("gpt-4o-mini", &function_frame).unwrap() > 0);
        assert!(streaming_output_tokens("gpt-4o-mini", &tool_frame).unwrap() > 0);

        let unknown = serde_json::json!({
            "choices": [{"delta": {"content": "", "new_billable_field": "hidden"}}]
        });
        assert!(streaming_output_tokens("gpt-4o-mini", &unknown).is_err());
    }

    #[test]
    fn multiple_plain_choices_are_tokenized_independently() {
        let model = "gpt-4o-mini";
        let (left, right) = find_choice_boundary_case(model);
        let choices = vec![
            serde_json::json!({"delta": {"content": left}}),
            serde_json::json!({"delta": {"content": right}}),
        ];
        let expected = token_count(model, left) + token_count(model, right);
        let old_concatenated = token_count(model, &format!("{left}{right}"));

        assert_eq!(
            locally_accounted_choice_tokens(model, &choices, "delta"),
            Ok(expected)
        );
        assert!(
            expected > old_concatenated,
            "the regression case must detect cross-choice BPE merging"
        );
    }

    #[test]
    fn multiple_tool_call_choices_are_tokenized_independently() {
        let model = "gpt-4o-mini";
        let first = serde_json::json!({"tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": {"name": "forecast", "arguments": "{\"city\":\"Bangkok\"}"}
        }]});
        let second = serde_json::json!({"function_call": {
            "name": "clock", "arguments": "{\"zone\":\"UTC\"}"
        }});
        let choices = vec![
            serde_json::json!({"delta": first.clone()}),
            serde_json::json!({"delta": second.clone()}),
        ];
        let expected = independently_accounted_generated_tokens(model, &first)
            + independently_accounted_generated_tokens(model, &second);

        assert_eq!(
            locally_accounted_choice_tokens(model, &choices, "delta"),
            Ok(expected)
        );
    }

    #[test]
    fn streaming_multiple_choices_use_independent_accumulation() {
        let model = "gpt-4o-mini";
        let (left, right) = find_choice_boundary_case(model);
        let frame = serde_json::json!({
            "choices": [
                {"delta": {"content": left}},
                {"delta": {"content": right}}
            ]
        });

        assert_eq!(
            streaming_output_tokens(model, &frame),
            Ok(token_count(model, left) + token_count(model, right))
        );
    }

    #[test]
    fn non_stream_multiple_choices_without_usage_use_independent_fallback() {
        let model = "gpt-4o-mini";
        let (left, right) = find_choice_boundary_case(model);
        let response = serde_json::json!({
            "choices": [
                {"message": {"role": "assistant", "content": left}},
                {"message": {"role": "assistant", "content": right}}
            ]
        });
        let expected = token_count(model, left) + token_count(model, right);

        assert_eq!(
            non_stream_output_tokens(model, &response),
            Ok((expected, "local supported-message tokenizer fallback"))
        );
    }

    #[test]
    fn mixed_plain_and_tool_choices_are_summed_independently() {
        let model = "gpt-4o-mini";
        let tool = serde_json::json!({"tool_calls": [{
            "index": 0, "id": "call_mixed", "type": "function",
            "function": {"name": "lookup", "arguments": "{\"id\":7}"}
        }]});
        let choices = vec![
            serde_json::json!({"delta": {"content": "answer"}}),
            serde_json::json!({"delta": tool.clone()}),
        ];
        let expected =
            token_count(model, "answer") + independently_accounted_generated_tokens(model, &tool);

        assert_eq!(
            locally_accounted_choice_tokens(model, &choices, "delta"),
            Ok(expected)
        );
    }

    #[test]
    fn unsupported_field_in_any_choice_fails_the_entire_accounting_operation() {
        let choices = vec![
            serde_json::json!({"delta": {"content": "supported"}}),
            serde_json::json!({"delta": {
                "content": "",
                "unknown_billable_output": "must fail closed"
            }}),
        ];

        assert_eq!(
            locally_accounted_choice_tokens("gpt-4o-mini", &choices, "delta"),
            Err("unsupported billable output field")
        );
    }

    #[test]
    fn completion_choice_accumulation_overflow_fails_closed() {
        assert_eq!(
            checked_add_completion_tokens(usize::MAX, 1),
            Err("completion token count overflowed")
        );
        assert_eq!(
            checked_add_completion_tokens(usize::MAX - 1, 1),
            Ok(usize::MAX)
        );
    }

    #[test]
    fn non_stream_tool_calls_use_usage_or_supported_local_fallback() {
        let response = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": {"name": "forecast", "arguments": "{\"city\":\"Bangkok\"}"}
            }]}}],
            "usage": {"completion_tokens": 17}
        });
        assert_eq!(
            non_stream_output_tokens("gpt-4o-mini", &response),
            Ok((17, "provider usage.completion_tokens"))
        );

        let mut without_usage = response;
        without_usage.as_object_mut().unwrap().remove("usage");
        let (tokens, source) = non_stream_output_tokens("gpt-4o-mini", &without_usage).unwrap();
        assert!(tokens > 0);
        assert_eq!(source, "local supported-message tokenizer fallback");
    }

    #[tokio::test]
    async fn proxy_auth_and_mock_mode_fail_before_body_or_budget() {
        let mut protected = test_state(0, 1.0);
        protected.proxy_token = Some(Arc::from("proxy-secret"));

        let response = call_proxy_with_body(
            protected.clone(),
            "auth-user",
            Body::from("this body must not be parsed"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            protected
                .budget_ledger
                .user_snapshot("auth-user")
                .total_spend,
            0.0
        );

        let mut wrong = proxy_headers("auth-user");
        wrong.insert("x-kilovolt-key", HeaderValue::from_static("wrong"));
        assert_eq!(
            chat_completions_proxy(State(protected.clone()), wrong, proxy_body())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let mut disabled = test_state(0, 1.0);
        disabled.mock_upstream_enabled = false;
        assert_eq!(
            mock_chat_completions(State(disabled), HeaderMap::new(), Body::from("{}"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let mut mock_headers = HeaderMap::new();
        mock_headers.insert("x-kilovolt-key", HeaderValue::from_static("proxy-secret"));
        assert_eq!(
            mock_chat_completions(
                State(protected.clone()),
                mock_headers,
                Body::from("{\"stream\":false}")
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            mock_chat_completions(State(protected), HeaderMap::new(), Body::from("{}"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn evaluation_proxy_requires_setup_and_the_generated_gateway_key() {
        let pending = test_evaluation_state(0);
        let response = chat_completions_proxy(
            State(pending.clone()),
            HeaderMap::new(),
            Body::from("body must not be parsed before setup"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(pending.budget_ledger.project_snapshot().total_spend, 0.0);

        let configured = test_evaluation_state(0);
        assert!(configured.complete_evaluation_setup(
            Arc::from("sk-provider-secret"),
            Arc::from("kvlt_generated_gateway"),
            10.0,
            1.0,
        ));
        let mut headers = proxy_headers("evaluation-user");
        headers.remove("x-mock-upstream");
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong-gateway"),
        );
        let response = chat_completions_proxy(
            State(configured.clone()),
            headers,
            Body::from("body must not be parsed with a bad gateway key"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(configured.budget_ledger.project_snapshot().total_spend, 0.0);

        let mut gemini_headers = proxy_headers("evaluation-user");
        gemini_headers.remove("x-mock-upstream");
        gemini_headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer kvlt_generated_gateway"),
        );
        let gemini_response = chat_completions_proxy(
            State(configured.clone()),
            gemini_headers,
            Body::from(
                serde_json::json!({
                    "model": "gemini-2.5-flash",
                    "messages": [{"role": "user", "content": "test"}],
                    "stream": true
                })
                .to_string(),
            ),
        )
        .await;
        assert_eq!(gemini_response.status(), StatusCode::BAD_REQUEST);
        let gemini_body = gemini_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert!(String::from_utf8_lossy(&gemini_body).contains("evaluation_openai_only"));
        assert_eq!(configured.budget_ledger.project_snapshot().total_spend, 0.0);

        for (project_limit, user_limit, expected_message) in [
            (0.0, 1.0, "Project Budget Exceeded"),
            (1.0, 0.0, "User Budget Exceeded"),
        ] {
            let limited = test_evaluation_state(0);
            assert!(limited.complete_evaluation_setup(
                Arc::from("sk-provider-secret"),
                Arc::from("kvlt_generated_gateway"),
                project_limit,
                user_limit,
            ));
            let mut headers = proxy_headers("evaluation-user");
            headers.remove("x-mock-upstream");
            headers.insert(
                axum::http::header::AUTHORIZATION,
                HeaderValue::from_static("Bearer kvlt_generated_gateway"),
            );
            let response =
                chat_completions_proxy(State(limited.clone()), headers, proxy_body()).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(expected_message));
            assert_eq!(limited.budget_ledger.project_snapshot().total_spend, 0.0);
            assert_eq!(
                limited
                    .budget_ledger
                    .user_snapshot("evaluation-user")
                    .total_spend,
                0.0
            );
        }
    }

    #[tokio::test]
    async fn unknown_model_never_contacts_upstream_or_reserves_budget() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::OK }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut state = test_state(port, 1.0);
        state.openai_upstream_url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        let mut headers = proxy_headers("unknown-model");
        headers.remove("x-mock-upstream");
        let body = Body::from(
            serde_json::json!({
                "model": "unpriced-model",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            })
            .to_string(),
        );
        let response = chat_completions_proxy(State(state.clone()), headers, body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("unknown-model")
                .total_spend,
            0.0
        );
        server.abort();
    }

    #[tokio::test]
    async fn valid_proxy_token_is_not_forwarded_and_default_bound_is_injected() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<(HeaderMap, Bytes)>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, body: Bytes| {
                let sender = sender.clone();
                async move {
                    sender.send((headers, body)).unwrap();
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"ok\"}}],\"usage\":{\"completion_tokens\":1}}",
                    )
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut state = test_state(port, 1.0);
        state.openai_upstream_url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        state.proxy_token = Some(Arc::from("proxy-secret"));
        state.non_stream_default_max_output_tokens = Some(12);

        let mut missing_token_headers = proxy_headers("unauthenticated-user");
        missing_token_headers.remove("x-mock-upstream");
        let rejected = chat_completions_proxy(
            State(state.clone()),
            missing_token_headers,
            proxy_body_with_stream(false),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("unauthenticated-user")
                .total_spend,
            0.0
        );

        let mut headers = proxy_headers("authenticated-user");
        headers.remove("x-mock-upstream");
        headers.insert("x-kilovolt-key", HeaderValue::from_static("proxy-secret"));
        let response =
            chat_completions_proxy(State(state.clone()), headers, proxy_body_with_stream(false))
                .await;
        assert_eq!(response.status(), StatusCode::OK);

        let (upstream_headers, upstream_body) = receiver.recv().await.unwrap();
        assert!(!upstream_headers.contains_key("x-kilovolt-key"));
        let forwarded: serde_json::Value = serde_json::from_slice(&upstream_body).unwrap();
        assert_eq!(forwarded["max_completion_tokens"], 12);
        assert!(
            state
                .budget_ledger
                .user_snapshot("authenticated-user")
                .committed_spend
                > 0.0
        );
        server.abort();
    }

    #[tokio::test]
    async fn missing_non_stream_bound_fails_before_upstream() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::OK }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut state = test_state(port, 1.0);
        state.openai_upstream_url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        state.non_stream_default_max_output_tokens = None;
        let mut headers = proxy_headers("missing-bound");
        headers.remove("x-mock-upstream");
        let response =
            chat_completions_proxy(State(state.clone()), headers, proxy_body_with_stream(false))
                .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("missing-bound")
                .total_spend,
            0.0
        );
        server.abort();
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
    async fn non_stream_upstream_failure_releases_prompt_and_output_reservations() {
        let (port, server) = spawn_mock_upstream(StatusCode::UNAUTHORIZED).await;
        let state = test_state(port, 1.0);
        let response = call_proxy_with_body(
            state.clone(),
            "non-stream-error",
            proxy_body_with_stream(false),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let snapshot = state.budget_ledger.user_snapshot("non-stream-error");
        assert_eq!(snapshot.committed_spend, 0.0);
        assert_eq!(snapshot.reserved_spend, 0.0);
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
        assert_eq!(
            state
                .budget_ledger
                .user_snapshot("json-user")
                .reserved_spend,
            0.0
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
            state.budget_ledger.user_snapshot("json-cutoff").total_spend,
            0.0,
            "the combined prompt/output reservation must fail before upstream"
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
    async fn provider_usage_above_bound_commits_full_output_reservation() {
        const UPSTREAM: &str = "{\"choices\":[{\"message\":{\"content\":\"too much\"}}],\"usage\":{\"completion_tokens\":2}}";
        let (port, server) =
            spawn_static_upstream(StatusCode::OK, "application/json", UPSTREAM).await;
        let state = test_state(port, 1.0);
        let body = Body::from(
            serde_json::json!({
                "model": "gpt-4o-mini",
                "messages": [{"role": "user", "content": "test prompt"}],
                "stream": false,
                "max_completion_tokens": 1
            })
            .to_string(),
        );
        let response = call_proxy_with_body(state.clone(), "bound-violation", body).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let expected = proxy_prompt_cost() + get_model_pricing("gpt-4o-mini").output_cost_per_token;
        let user = state.budget_ledger.user_snapshot("bound-violation");
        assert!((user.committed_spend - expected).abs() < 1e-15);
        assert_eq!(user.reserved_spend, 0.0);
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
    async fn non_stream_cancellation_after_acceptance_commits_full_reservation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/mock/v1/chat/completions",
            post(|| async {
                let body = Body::from_stream(stream::once(async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok::<Bytes, std::io::Error>(Bytes::from_static(
                        b"{\"choices\":[{\"message\":{\"content\":\"late\"}}]}",
                    ))
                }));
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap()
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state(port, 1.0);
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            call_proxy_with_body(
                task_state,
                "cancel-non-stream",
                proxy_body_with_stream(false),
            )
            .await
        });
        for _ in 0..1_000 {
            let snapshot = state.budget_ledger.user_snapshot("cancel-non-stream");
            if snapshot.committed_spend > 0.0 && snapshot.reserved_spend > 0.0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let accepted = state.budget_ledger.user_snapshot("cancel-non-stream");
        assert!(accepted.committed_spend > 0.0);
        assert!(accepted.reserved_spend > 0.0);
        task.abort();
        let _ = task.await;
        let finalized = state.budget_ledger.user_snapshot("cancel-non-stream");
        assert_eq!(finalized.reserved_spend, 0.0);
        assert!(finalized.committed_spend > accepted.committed_spend);
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
