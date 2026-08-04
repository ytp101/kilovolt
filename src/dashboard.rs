use crate::config::{AppState, EvaluationTestResult, RecentRequest, secrets_match};
use crate::proxy::chat_completions_proxy;
use axum::{
    Form, Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const APP_CSS: &str = include_str!("ui/app.css");
const SETUP_HTML: &str = include_str!("ui/setup.html");
const ONBOARDING_HTML: &str = include_str!("ui/onboarding.html");
const DASHBOARD_HTML: &str = include_str!("ui/dashboard.html");
const CONNECT_HTML: &str = include_str!("ui/connect.html");
const DOCUMENTATION_HTML: &str = include_str!("ui/documentation.html");
const EVALUATION_USER_ID: &str = "kilovolt-evaluation";

#[derive(serde::Deserialize)]
pub struct SetupForm {
    provider_api_key: String,
    project_budget: String,
    default_budget: String,
}

#[derive(serde::Deserialize)]
pub struct BudgetUpdateForm {
    project_budget: String,
    default_budget: String,
}

#[derive(serde::Serialize)]
struct BudgetUpdatePayload {
    ok: bool,
    message: String,
    project_budget_usd: f64,
    default_budget_usd: f64,
    current_project_spend_usd: f64,
}

#[derive(serde::Serialize)]
struct StatsPayload {
    health: HealthStats,
    budget: BudgetStats,
    ledger: LedgerMetadata,
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
    recent_accepted_requests: usize,
    recent_blocked_requests: usize,
    project_budget_usd: f64,
    current_project_spend_usd: f64,
    default_budget_usd: f64,
    recent_requests: Vec<RecentRequest>,
    current_spend_by_user: HashMap<String, f64>,
}

#[derive(serde::Serialize)]
struct LedgerMetadata {
    persistence: &'static str,
    scope: &'static str,
    restart_resets_spend: bool,
    multi_instance_safe: bool,
}

#[derive(serde::Serialize)]
struct EvaluationTestPayload {
    ok: bool,
    status: u16,
    message: String,
    model: String,
    tokens: usize,
    input_tokens: usize,
    output_tokens: usize,
    output_text: String,
    spend_usd: f64,
    latency_ms: u64,
    project_budget_usd: f64,
    project_remaining_usd: f64,
    project_calculated_spend_usd: f64,
    user_calculated_spend_usd: f64,
}

fn get_memory_usage_kb() -> usize {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2
                        && let Ok(kb) = parts[1].parse::<usize>()
                    {
                        return kb;
                    }
                }
            }
        }
    }
    15360
}

fn dashboard_auth_failure(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if state.evaluation_mode() {
        return (!state.evaluation_setup_complete()).then(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "setup_required",
                    "message": "Complete local evaluation setup at /."
                })),
            )
                .into_response()
        });
    }

    let Some(expected_token) = state.dashboard_token.as_deref() else {
        return Some(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "dashboard_disabled",
                    "message": "Set KILOVOLT_DASHBOARD_TOKEN and restart Kilovolt to enable the dashboard."
                })),
            )
                .into_response(),
        );
    };

    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|authorization| {
            if let Some(token) = authorization.strip_prefix("Bearer ") {
                return secrets_match(expected_token, token);
            }

            let Some(encoded) = authorization.strip_prefix("Basic ") else {
                return false;
            };
            let Ok(decoded) = STANDARD.decode(encoded) else {
                return false;
            };
            let Ok(credentials) = std::str::from_utf8(&decoded) else {
                return false;
            };
            let Some((username, password)) = credentials.split_once(':') else {
                return false;
            };
            username == "kilovolt" && secrets_match(expected_token, password)
        });

    if authorized {
        None
    } else {
        let mut response = (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "unauthorized",
                "message": "Dashboard authentication is required."
            })),
        )
            .into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Kilovolt dashboard\""),
        );
        Some(response)
    }
}

/// REST endpoint `/api/stats` exposing process-local operational and budget state.
pub async fn get_stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = dashboard_auth_failure(&state, &headers) {
        return response;
    }

    let total_requests = state.total_requests.load(Ordering::Relaxed);
    let total_latency = state.total_latency_ms.load(Ordering::Relaxed);
    let avg_latency = if total_requests > 0 {
        total_latency as f64 / total_requests as f64
    } else {
        0.0
    };
    let recent = state
        .recent_requests
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let ledger = state.budget_ledger.snapshot();
    let project = state.budget_ledger.project_snapshot();
    let (project_budget, default_budget) = state.effective_budgets();
    let recent_accepted_requests = recent.iter().filter(|request| request.status < 400).count();
    let recent_blocked_requests = recent
        .iter()
        .filter(|request| request.status == StatusCode::TOO_MANY_REQUESTS.as_u16())
        .count();

    let payload = StatsPayload {
        health: HealthStats {
            uptime_seconds: state.start_time.elapsed().as_secs(),
            memory_usage_kb: get_memory_usage_kb(),
            avg_latency_ms: avg_latency,
        },
        budget: BudgetStats {
            total_tokens_consumed: state.total_tokens_consumed.load(Ordering::Relaxed),
            recent_accepted_requests,
            recent_blocked_requests,
            project_budget_usd: project_budget,
            current_project_spend_usd: project.committed_spend,
            default_budget_usd: default_budget,
            recent_requests: recent,
            current_spend_by_user: ledger,
        },
        ledger: LedgerMetadata {
            persistence: "memory",
            scope: "process",
            restart_resets_spend: true,
            multi_instance_safe: false,
        },
    };

    (StatusCode::OK, Json(payload)).into_response()
}

fn with_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn render_template(template: &str) -> String {
    template.replace("{{APP_CSS}}", APP_CSS)
}

fn setup_page(error: Option<&str>) -> Response {
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<div class=\"error\" role=\"alert\">{}</div>",
            escape_html(message)
        )
    });
    let html = render_template(SETUP_HTML).replace("{{SETUP_ERROR}}", &error);
    with_no_store(Html(html).into_response())
}

fn parse_budget(value: &str, label: &str) -> Result<f64, String> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a number in USD."))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!(
            "{label} must be a finite amount greater than or equal to zero."
        ));
    }
    Ok(parsed)
}

fn evaluation_gateway_authorized(gateway_key: &str, browser_headers: &HeaderMap) -> bool {
    browser_headers
        .get("x-kilovolt-evaluation-key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| secrets_match(gateway_key, actual))
}

fn budget_update_payload(state: &AppState, ok: bool, message: String) -> BudgetUpdatePayload {
    let (project_budget, default_budget) = state.effective_budgets();
    BudgetUpdatePayload {
        ok,
        message,
        project_budget_usd: project_budget,
        default_budget_usd: default_budget,
        current_project_spend_usd: state.budget_ledger.project_snapshot().committed_spend,
    }
}

fn generate_gateway_key() -> String {
    format!(
        "kvlt_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn masked_secret(secret: &str, prefix: &str) -> String {
    let suffix = secret.chars().rev().take(4).collect::<Vec<_>>();
    let suffix = suffix.into_iter().rev().collect::<String>();
    format!("{prefix}••••{suffix}")
}

fn masked_provider_key(provider_api_key: &str) -> String {
    masked_secret(provider_api_key, "")
}

fn masked_gateway_key(gateway_key: &str) -> String {
    let prefix = gateway_key.chars().take(9).collect::<String>();
    let suffix = gateway_key
        .chars()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}••••••••••••••••{suffix}")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

fn format_decimal(value: f64, precision: usize) -> String {
    let formatted = format!("{value:.precision$}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn format_spend(value: f64) -> String {
    if value == 0.0 {
        "$0".to_string()
    } else {
        let precision = if value.abs() < 0.0001 { 7 } else { 5 };
        format!("${}", format_decimal(value, precision))
    }
}

fn format_budget(value: f64) -> String {
    format!("${value:.2}")
}

fn format_remaining(value: f64) -> String {
    format!("${}", format_decimal(value, 7))
}

fn format_latency(milliseconds: u64) -> String {
    if milliseconds >= 1_000 {
        format!("{} s", format_decimal(milliseconds as f64 / 1_000.0, 2))
    } else {
        format!("{milliseconds} ms")
    }
}

fn latest_evaluation_request(state: &AppState) -> Option<RecentRequest> {
    state
        .recent_requests
        .lock()
        .unwrap()
        .iter()
        .find(|request| request.user_id == EVALUATION_USER_ID)
        .cloned()
}

fn render_onboarding(state: &AppState) -> String {
    let setup = state
        .evaluation_setup_snapshot()
        .expect("onboarding requires completed evaluation setup");
    let (project_budget, user_budget) = setup.budgets();
    let result = state.evaluation_test_result();
    let success_hidden = if result.is_some() { "" } else { "hidden" };
    let verify_action_hidden = if result.is_some() { "hidden" } else { "" };
    let result_output_hidden = if result
        .as_ref()
        .is_some_and(|result| !result.output_text.is_empty())
    {
        ""
    } else {
        "hidden"
    };
    let result_model = result
        .as_ref()
        .map(|result| escape_html(&result.model))
        .unwrap_or_default();
    let result_input_tokens = result
        .as_ref()
        .map(|result| result.input_tokens.to_string())
        .unwrap_or_default();
    let result_output_tokens = result
        .as_ref()
        .map(|result| result.output_tokens.to_string())
        .unwrap_or_default();
    let result_output = result
        .as_ref()
        .map(|result| escape_html(&result.output_text))
        .unwrap_or_default();
    let result_spend = result
        .as_ref()
        .map(|result| format_spend(result.spend_usd))
        .unwrap_or_default();
    let result_latency = result
        .as_ref()
        .map(|result| format_latency(result.latency_ms))
        .unwrap_or_default();
    let result_project_spend = result
        .as_ref()
        .map(|result| result.project_calculated_spend_usd)
        .unwrap_or_default();
    let result_project_remaining = (project_budget - result_project_spend).max(0.0);

    render_template(ONBOARDING_HTML)
        .replace(
            "{{PROVIDER_KEY}}",
            &escape_html(&masked_provider_key(setup.provider_api_key())),
        )
        .replace("{{PROJECT_BUDGET}}", &format_budget(project_budget))
        .replace("{{USER_BUDGET}}", &format_budget(user_budget))
        .replace("{{PROJECT_BUDGET_VALUE}}", &format!("{project_budget:.2}"))
        .replace("{{USER_BUDGET_VALUE}}", &format!("{user_budget:.2}"))
        .replace(
            "{{VERIFICATION_COMPLETE}}",
            if state.evaluation_test_succeeded() {
                "true"
            } else {
                "false"
            },
        )
        .replace("{{VERIFY_ACTION_HIDDEN}}", verify_action_hidden)
        .replace("{{SUCCESS_HIDDEN}}", success_hidden)
        .replace("{{RESULT_OUTPUT_HIDDEN}}", result_output_hidden)
        .replace("{{RESULT_MODEL}}", &result_model)
        .replace("{{RESULT_INPUT_TOKENS}}", &result_input_tokens)
        .replace("{{RESULT_OUTPUT_TOKENS}}", &result_output_tokens)
        .replace("{{RESULT_OUTPUT}}", &result_output)
        .replace("{{RESULT_SPEND}}", &result_spend)
        .replace("{{RESULT_LATENCY}}", &result_latency)
        .replace(
            "{{RESULT_PROJECT_REMAINING}}",
            &format_remaining(result_project_remaining),
        )
        .replace(
            "{{RESULT_PROJECT_SPEND}}",
            &format_spend(result_project_spend),
        )
        .replace(
            "{{RESULT_PROJECT_SPEND_VALUE}}",
            &result_project_spend.to_string(),
        )
        .replace("{{GATEWAY_KEY}}", &escape_html(setup.gateway_key()))
}

fn render_dashboard(state: &AppState) -> String {
    let (brand_href, secondary_nav, edit_limits_link, connect_panel) = if state.evaluation_mode() {
        let setup = state
            .evaluation_setup_snapshot()
            .expect("evaluation dashboard requires completed setup");
        let connect_panel = CONNECT_HTML
            .replace("{{GATEWAY_KEY}}", &escape_html(setup.gateway_key()))
            .replace(
                "{{MASKED_GATEWAY_KEY}}",
                &escape_html(&masked_gateway_key(setup.gateway_key())),
            );
        (
            "/dashboard",
            "<a href=\"/dashboard#transactions-heading\">Transactions</a>",
            "· <a href=\"/#budget-editor\">Edit limits</a>",
            connect_panel,
        )
    } else {
        ("/dashboard", "", "", String::new())
    };
    render_template(DASHBOARD_HTML)
        .replace("{{BRAND_HREF}}", brand_href)
        .replace("{{GETTING_STARTED_NAV}}", secondary_nav)
        .replace("{{MONITOR_PROGRESS}}", "")
        .replace("{{EDIT_LIMITS_LINK}}", edit_limits_link)
        .replace("{{CONNECT_PANEL}}", &connect_panel)
}

fn render_documentation(state: &AppState) -> String {
    let (brand_href, getting_started_nav) = if state.evaluation_mode() {
        (
            "/dashboard",
            "<a href=\"/dashboard#transactions-heading\">Transactions</a>",
        )
    } else {
        ("/dashboard", "")
    };
    render_template(DOCUMENTATION_HTML)
        .replace("{{BRAND_HREF}}", brand_href)
        .replace("{{GETTING_STARTED_NAV}}", getting_started_nav)
}

fn evaluation_payload(
    state: &AppState,
    ok: bool,
    status: StatusCode,
    message: String,
) -> EvaluationTestPayload {
    let result = state.evaluation_test_result();
    let (project_budget, _) = state.effective_budgets();
    let project_calculated_spend_usd = state.budget_ledger.project_snapshot().committed_spend;
    EvaluationTestPayload {
        ok,
        status: status.as_u16(),
        message,
        model: result
            .as_ref()
            .map(|result| result.model.clone())
            .unwrap_or_default(),
        tokens: result
            .as_ref()
            .map_or(0, |result| result.input_tokens + result.output_tokens),
        input_tokens: result.as_ref().map_or(0, |result| result.input_tokens),
        output_tokens: result.as_ref().map_or(0, |result| result.output_tokens),
        output_text: result
            .as_ref()
            .map(|result| result.output_text.clone())
            .unwrap_or_default(),
        spend_usd: result.as_ref().map_or(0.0, |result| result.spend_usd),
        latency_ms: result.as_ref().map_or(0, |result| result.latency_ms),
        project_budget_usd: project_budget,
        project_remaining_usd: (project_budget - project_calculated_spend_usd).max(0.0),
        project_calculated_spend_usd,
        user_calculated_spend_usd: state
            .budget_ledger
            .user_snapshot(EVALUATION_USER_ID)
            .committed_spend,
    }
}

fn actionable_test_error(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            "OpenAI rejected the saved API key. Restart Kilovolt, enter a valid key, and try again."
                .to_string()
        }
        StatusCode::TOO_MANY_REQUESTS => {
            "The request was blocked by a spending limit or provider rate limit. Check the configured limits and OpenAI account, then try again."
                .to_string()
        }
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            "Kilovolt could not reach OpenAI. Check the network connection and try again."
                .to_string()
        }
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            "OpenAI rejected the test request. Check that the saved key can use gpt-4o-mini, then try again."
                .to_string()
        }
        _ => format!(
            "The provider request failed with HTTP {}. Check provider access and try again.",
            status.as_u16()
        ),
    }
}

/// Sends a small paid request through the same proxy and accounting handler used
/// by customer applications, then returns only a sanitized result summary.
pub async fn post_evaluation_test(
    State(state): State<AppState>,
    browser_headers: HeaderMap,
) -> Response {
    let Some(setup) = state.evaluation_setup_snapshot() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let authorized = evaluation_gateway_authorized(setup.gateway_key(), &browser_headers);
    if !authorized {
        return with_no_store(
            (
                StatusCode::UNAUTHORIZED,
                Json(evaluation_payload(
                    &state,
                    false,
                    StatusCode::UNAUTHORIZED,
                    "Kilovolt gateway key authentication failed.".to_string(),
                )),
            )
                .into_response(),
        );
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", setup.gateway_key()))
            .expect("generated evaluation gateway key is a valid header"),
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert("x-user-id", HeaderValue::from_static(EVALUATION_USER_ID));
    let request = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "Reply with: Kilovolt is working."}],
        "stream": false,
        "max_completion_tokens": 16
    });

    let proxy_response = chat_completions_proxy(
        State(state.clone()),
        headers,
        Body::from(request.to_string()),
    )
    .await;
    let status = proxy_response.status();
    let response_bytes = match proxy_response.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return with_no_store(
                (
                    StatusCode::BAD_GATEWAY,
                    Json(evaluation_payload(
                        &state,
                        false,
                        StatusCode::BAD_GATEWAY,
                        "The test response could not be read safely. Check the provider connection and try again."
                            .to_string(),
                    )),
                )
                    .into_response(),
            );
        }
    };

    let ok = status.is_success();
    let recorded_request = latest_evaluation_request(&state);
    let message = if ok {
        "Kilovolt is working. You can now connect your application.".to_string()
    } else {
        actionable_test_error(status)
    };

    let mut payload = evaluation_payload(&state, ok, status, message);
    if let Some(record) = recorded_request {
        payload.model = record.model;
        payload.tokens = record.tokens;
        payload.spend_usd = record.cost;
        payload.latency_ms = record.duration_ms;
    }
    if ok
        && let Ok(provider_response) = serde_json::from_slice::<serde_json::Value>(&response_bytes)
    {
        if let Some(model) = provider_response
            .get("model")
            .and_then(serde_json::Value::as_str)
        {
            payload.model = model.to_string();
        }
        if let Some(tokens) = provider_response
            .pointer("/usage/total_tokens")
            .and_then(serde_json::Value::as_u64)
            .and_then(|tokens| usize::try_from(tokens).ok())
        {
            payload.tokens = tokens;
        }
        if let Some(tokens) = provider_response
            .pointer("/usage/prompt_tokens")
            .and_then(serde_json::Value::as_u64)
            .and_then(|tokens| usize::try_from(tokens).ok())
        {
            payload.input_tokens = tokens;
        }
        if let Some(tokens) = provider_response
            .pointer("/usage/completion_tokens")
            .and_then(serde_json::Value::as_u64)
            .and_then(|tokens| usize::try_from(tokens).ok())
        {
            payload.output_tokens = tokens;
        }
        if let Some(output_text) = provider_response
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
        {
            payload.output_text = output_text.to_string();
        }
    }
    if ok {
        state.save_evaluation_test_result(EvaluationTestResult {
            model: payload.model.clone(),
            input_tokens: payload.input_tokens,
            output_tokens: payload.output_tokens,
            output_text: payload.output_text.clone(),
            spend_usd: payload.spend_usd,
            latency_ms: payload.latency_ms,
            project_calculated_spend_usd: payload.project_calculated_spend_usd,
        });
    }

    with_no_store((status, Json(payload)).into_response())
}

/// Root route for the guided local evaluation journey.
pub async fn get_root(State(state): State<AppState>) -> Response {
    if !state.evaluation_mode() {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, "/dashboard")]).into_response();
    }
    if !state.evaluation_setup_complete() {
        return setup_page(None);
    }
    with_no_store(Html(render_onboarding(&state)).into_response())
}

/// Completes the one-time, in-memory Docker evaluation setup.
pub async fn post_setup(State(state): State<AppState>, Form(form): Form<SetupForm>) -> Response {
    if !state.evaluation_mode() || state.evaluation_setup_complete() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let provider_api_key = form.provider_api_key.trim();
    if provider_api_key.is_empty() {
        let mut response = setup_page(Some("OpenAI API key must not be empty."));
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return response;
    }
    if HeaderValue::from_str(&format!("Bearer {provider_api_key}")).is_err() {
        let mut response = setup_page(Some("OpenAI API key contains invalid characters."));
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return response;
    }

    let project_budget = match parse_budget(&form.project_budget, "Project limit") {
        Ok(value) => value,
        Err(message) => {
            let mut response = setup_page(Some(&message));
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return response;
        }
    };
    let default_budget = match parse_budget(&form.default_budget, "Default per-user limit") {
        Ok(value) => value,
        Err(message) => {
            let mut response = setup_page(Some(&message));
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return response;
        }
    };

    if !state.complete_evaluation_setup(
        Arc::from(provider_api_key),
        Arc::from(generate_gateway_key()),
        project_budget,
        default_budget,
    ) {
        return StatusCode::NOT_FOUND.into_response();
    }

    with_no_store((StatusCode::SEE_OTHER, [(header::LOCATION, "/#verify")]).into_response())
}

/// Updates both evaluation budget limits together without changing recorded spend.
pub async fn post_evaluation_budgets(
    State(state): State<AppState>,
    browser_headers: HeaderMap,
    Form(form): Form<BudgetUpdateForm>,
) -> Response {
    let Some(setup) = state.evaluation_setup_snapshot() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !evaluation_gateway_authorized(setup.gateway_key(), &browser_headers) {
        return with_no_store(
            (
                StatusCode::UNAUTHORIZED,
                Json(budget_update_payload(
                    &state,
                    false,
                    "Kilovolt gateway key authentication failed.".to_string(),
                )),
            )
                .into_response(),
        );
    }

    let project_budget = match parse_budget(&form.project_budget, "Project limit") {
        Ok(value) => value,
        Err(message) => {
            return with_no_store(
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(budget_update_payload(&state, false, message)),
                )
                    .into_response(),
            );
        }
    };
    let default_budget = match parse_budget(&form.default_budget, "Default per-user limit") {
        Ok(value) => value,
        Err(message) => {
            return with_no_store(
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(budget_update_payload(&state, false, message)),
                )
                    .into_response(),
            );
        }
    };

    if !state.update_evaluation_budgets(project_budget, default_budget) {
        return StatusCode::NOT_FOUND.into_response();
    }

    with_no_store(
        (
            StatusCode::OK,
            Json(budget_update_payload(
                &state,
                true,
                "Spending limits updated. Existing calculated spend was preserved.".to_string(),
            )),
        )
            .into_response(),
    )
}

/// Route handler for the authenticated operational dashboard.
pub async fn get_dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if state.evaluation_mode() && !state.evaluation_setup_complete() {
        return setup_page(None);
    }
    if let Some(response) = dashboard_auth_failure(&state, &headers) {
        return response;
    }

    with_no_store(Html(render_dashboard(&state)).into_response())
}

/// Route handler for the local integration quick reference.
pub async fn get_documentation(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.evaluation_mode()
        && let Some(response) = dashboard_auth_failure(&state, &headers)
    {
        return response;
    }

    with_no_store(Html(render_documentation(&state)).into_response())
}

#[cfg(test)]
mod tests {
    use super::{
        BudgetUpdateForm, SetupForm, generate_gateway_key, get_dashboard, get_documentation,
        get_root, get_stats, post_evaluation_budgets, post_evaluation_test, post_setup,
    };
    use axum::extract::{Form, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::routing::post;
    use axum::{Json, Router};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use http_body_util::BodyExt;
    use std::sync::Arc;

    use crate::config::{test_evaluation_state, test_state};
    use crate::ledger::{BudgetError, BudgetScope};

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("valid test header"),
        );
        headers
    }

    #[test]
    fn generated_gateway_keys_are_high_entropy_and_unique() {
        let first = generate_gateway_key();
        let second = generate_gateway_key();
        assert!(first.starts_with("kvlt_"));
        assert_eq!(first.len(), 69);
        assert!(first[5..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn dashboard_and_stats_reject_unauthenticated_requests() {
        let state = test_state(0, 1.0);
        let dashboard = get_dashboard(State(state.clone()), HeaderMap::new()).await;
        let stats = get_stats(State(state), HeaderMap::new()).await;

        assert_eq!(dashboard.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(stats.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            dashboard
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Basic realm=\"Kilovolt dashboard\"")
        );
    }

    #[tokio::test]
    async fn dashboard_and_stats_accept_bearer_authentication() {
        let state = test_state(0, 1.0);
        let headers = bearer_headers("test-dashboard-token");
        let dashboard = get_dashboard(State(state.clone()), headers.clone()).await;
        let stats = get_stats(State(state), headers).await;

        assert_eq!(dashboard.status(), StatusCode::OK);
        assert_eq!(stats.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_stats_expose_process_local_ledger_metadata() {
        let state = test_state(0, 1.0);
        let response = get_stats(State(state), bearer_headers("test-dashboard-token")).await;
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["ledger"]["persistence"], "memory");
        assert_eq!(json["ledger"]["scope"], "process");
        assert_eq!(json["ledger"]["restart_resets_spend"], true);
        assert_eq!(json["ledger"]["multi_instance_safe"], false);
    }

    #[tokio::test]
    async fn dashboard_and_proxy_credentials_are_independent() {
        let mut state = test_state(0, 1.0);
        state.proxy_token = Some(Arc::from("proxy-only-secret"));
        assert_eq!(
            get_stats(State(state.clone()), bearer_headers("proxy-only-secret"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get_stats(State(state), bearer_headers("test-dashboard-token"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn browser_basic_authentication_is_supported() {
        let state = test_state(0, 1.0);
        let credentials = STANDARD.encode("kilovolt:test-dashboard-token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {credentials}")).expect("valid test header"),
        );

        assert_eq!(
            get_dashboard(State(state), headers).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn missing_dashboard_configuration_disables_the_endpoints() {
        let mut state = test_state(0, 1.0);
        state.dashboard_token = None;

        let dashboard = get_dashboard(State(state.clone()), HeaderMap::new()).await;
        let stats = get_stats(State(state), HeaderMap::new()).await;
        assert_eq!(dashboard.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(stats.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn local_documentation_contains_verified_examples_without_setup_secrets() {
        let state = test_evaluation_state(0);
        let response = get_documentation(State(state), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("Use Kilovolt from your backend"));
        assert!(html.contains("Python"));
        assert!(html.contains("JavaScript"));
        assert!(html.contains("curl http://127.0.0.1:8080/v1/chat/completions"));
        assert!(html.contains("KILOVOLT_API_KEY"));
        assert!(html.contains("X-User-ID"));
        assert!(html.contains("Full GitHub documentation"));
        assert!(!html.contains("kvlt_test_gateway"));
    }

    #[tokio::test]
    async fn manual_mode_documentation_uses_dashboard_authentication() {
        let state = test_state(0, 1.0);
        assert_eq!(
            get_documentation(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get_documentation(State(state), bearer_headers("test-dashboard-token"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn evaluation_setup_validates_input_masks_provider_and_closes_after_success() {
        let state = test_evaluation_state(0);
        let root = get_root(State(state.clone())).await;
        let root_body = root.into_body().collect().await.unwrap().to_bytes();
        let root_html = String::from_utf8_lossy(&root_body);
        assert!(root_html.contains("Continue to verification"));
        assert!(root_html.contains("Customize spending limits"));
        assert!(root_html.contains("https://platform.openai.com/api-keys"));
        assert!(root_html.contains("target=\"_blank\" rel=\"noopener noreferrer\""));

        let empty_key = post_setup(
            State(state.clone()),
            Form(SetupForm {
                provider_api_key: "  ".to_string(),
                project_budget: "10".to_string(),
                default_budget: "1".to_string(),
            }),
        )
        .await;
        assert_eq!(empty_key.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let invalid_key = post_setup(
            State(state.clone()),
            Form(SetupForm {
                provider_api_key: "sk-invalid\nheader".to_string(),
                project_budget: "10".to_string(),
                default_budget: "1".to_string(),
            }),
        )
        .await;
        assert_eq!(invalid_key.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let invalid_key_body = invalid_key.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&invalid_key_body).contains("sk-invalid"));

        let invalid_budget = post_setup(
            State(state.clone()),
            Form(SetupForm {
                provider_api_key: "sk-invalid-budget-secret".to_string(),
                project_budget: "NaN".to_string(),
                default_budget: "1".to_string(),
            }),
        )
        .await;
        assert_eq!(invalid_budget.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let invalid_body = invalid_budget
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert!(!String::from_utf8_lossy(&invalid_body).contains("sk-invalid-budget-secret"));

        let provider_key = "sk-provider-secret-ABCD";
        let configured = post_setup(
            State(state.clone()),
            Form(SetupForm {
                provider_api_key: provider_key.to_string(),
                project_budget: "7.5".to_string(),
                default_budget: "0.75".to_string(),
            }),
        )
        .await;
        assert_eq!(configured.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            configured
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            configured
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/#verify")
        );
        let configured = get_root(State(state.clone())).await;
        assert_eq!(configured.status(), StatusCode::OK);
        let configured_body = configured.into_body().collect().await.unwrap().to_bytes();
        let configured_html = String::from_utf8_lossy(&configured_body);
        assert!(!configured_html.contains(provider_key));
        assert!(configured_html.contains("••••ABCD"));
        assert!(configured_html.contains("Setup complete"));
        assert!(configured_html.contains("Send test request"));
        assert!(configured_html.contains("id=\"configure-stage\""));
        assert!(configured_html.contains("data-stage-target=\"configure\""));
        assert!(configured_html.contains("data-stage-target=\"verify\""));
        assert!(configured_html.contains("href=\"/dashboard#connect-app\""));
        assert!(configured_html.contains("Skip and connect app"));
        assert!(configured_html.contains("let verificationComplete = false;"));
        assert!(!configured_html.contains("{{VERIFICATION_COMPLETE}}"));
        assert!(!configured_html.contains("href=\"data:,\""));
        assert!(configured_html.contains("href=\"/documentation\""));

        let setup = state.evaluation_setup_snapshot().unwrap();
        assert!(setup.gateway_key().starts_with("kvlt_"));
        assert_eq!(setup.gateway_key().len(), 69);
        assert_eq!(configured_html.matches(setup.gateway_key()).count(), 1);
        assert_eq!(state.effective_budgets(), (7.5, 0.75));
        assert_eq!(
            get_stats(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::OK
        );

        let dashboard = get_dashboard(State(state.clone()), HeaderMap::new()).await;
        let dashboard_body = dashboard.into_body().collect().await.unwrap().to_bytes();
        let dashboard_html = String::from_utf8_lossy(&dashboard_body);
        assert!(dashboard_html.contains("Monitor spending"));
        assert!(dashboard_html.contains("Accepted"));
        assert!(dashboard_html.contains("Blocked"));
        assert!(dashboard_html.contains("Connect your application"));
        assert!(dashboard_html.contains("KILOVOLT_BASE_URL=http://127.0.0.1:8080/v1"));
        assert!(dashboard_html.contains("pip install openai python-dotenv"));
        assert!(dashboard_html.contains("load_dotenv()"));
        assert!(dashboard_html.contains("data-copy-target=\"python-code\""));
        assert!(dashboard_html.contains("X-User-ID</code> tells Kilovolt"));
        assert!(dashboard_html.contains("href=\"/documentation#trusted-user-identity\""));
        assert!(
            dashboard_html.find("Project spend").unwrap()
                < dashboard_html.find("System health").unwrap()
        );
        assert!(
            dashboard_html.find("Connect your application").unwrap()
                < dashboard_html.find("id=\"transactions-heading\"").unwrap()
        );
        assert_eq!(dashboard_html.matches(setup.gateway_key()).count(), 1);
        assert!(!dashboard_html.contains(provider_key));

        let closed = post_setup(
            State(state.clone()),
            Form(SetupForm {
                provider_api_key: "sk-replacement".to_string(),
                project_budget: "20".to_string(),
                default_budget: "2".to_string(),
            }),
        )
        .await;
        assert_eq!(closed.status(), StatusCode::NOT_FOUND);

        let later_root = get_root(State(state)).await;
        let later_body = later_root.into_body().collect().await.unwrap().to_bytes();
        assert!(!String::from_utf8_lossy(&later_body).contains(provider_key));
    }

    #[tokio::test]
    async fn evaluation_budget_update_is_authenticated_atomic_and_preserves_spend() {
        let state = test_evaluation_state(0);
        assert_eq!(
            post_evaluation_budgets(
                State(state.clone()),
                HeaderMap::new(),
                Form(BudgetUpdateForm {
                    project_budget: "2".to_string(),
                    default_budget: "0.5".to_string(),
                }),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert!(state.complete_evaluation_setup(
            Arc::from("sk-budget-update-secret"),
            Arc::from("kvlt_budget_update_gateway"),
            10.0,
            1.0,
        ));

        state
            .budget_ledger
            .reserve_prompt("existing-spend", "budget-user", 0.25, 10.0, 1.0)
            .unwrap();
        state
            .budget_ledger
            .commit_prompt("existing-spend", "budget-user")
            .unwrap();
        let project_before = state.budget_ledger.project_snapshot();
        let user_before = state.budget_ledger.user_snapshot("budget-user");

        let unauthorized = post_evaluation_budgets(
            State(state.clone()),
            HeaderMap::new(),
            Form(BudgetUpdateForm {
                project_budget: "2".to_string(),
                default_budget: "0.5".to_string(),
            }),
        )
        .await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(state.effective_budgets(), (10.0, 1.0));

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-kilovolt-evaluation-key",
            HeaderValue::from_static("kvlt_budget_update_gateway"),
        );
        let invalid = post_evaluation_budgets(
            State(state.clone()),
            headers.clone(),
            Form(BudgetUpdateForm {
                project_budget: "not-a-budget".to_string(),
                default_budget: "0.5".to_string(),
            }),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(state.effective_budgets(), (10.0, 1.0));

        let updated = post_evaluation_budgets(
            State(state.clone()),
            headers,
            Form(BudgetUpdateForm {
                project_budget: "0.10".to_string(),
                default_budget: "0.10".to_string(),
            }),
        )
        .await;
        assert_eq!(updated.status(), StatusCode::OK);
        let body = updated.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["project_budget_usd"], 0.10);
        assert_eq!(payload["default_budget_usd"], 0.10);
        assert_eq!(payload["current_project_spend_usd"], 0.25);
        assert_eq!(state.budget_ledger.project_snapshot(), project_before);
        assert_eq!(
            state.budget_ledger.user_snapshot("budget-user"),
            user_before
        );

        let (project_budget, user_budget) = state.effective_budgets();
        assert_eq!(
            state.budget_ledger.reserve_prompt(
                "future-request",
                "budget-user",
                0.01,
                project_budget,
                user_budget,
            ),
            Err(BudgetError::BudgetExceeded(BudgetScope::Project))
        );
        assert_eq!(state.budget_ledger.project_snapshot(), project_before);
        assert_eq!(
            state.budget_ledger.user_snapshot("budget-user"),
            user_before
        );
    }

    #[tokio::test]
    async fn evaluation_test_uses_provider_key_and_returns_recorded_result() {
        let (authorization_sender, mut authorization_receiver) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap| {
                let authorization_sender = authorization_sender.clone();
                async move {
                    let authorization = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    authorization_sender.send(authorization).unwrap();
                    Json(serde_json::json!({
                        "id": "chatcmpl-evaluation-test",
                        "model": "gpt-4o-mini-2024-07-18",
                        "choices": [{"message": {"role": "assistant", "content": "Kilovolt is working."}}],
                        "usage": {"prompt_tokens": 8, "completion_tokens": 4, "total_tokens": 12}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut state = test_evaluation_state(0);
        state.openai_upstream_url = format!("http://{address}/v1/chat/completions");
        assert!(state.complete_evaluation_setup(
            Arc::from("sk-provider-upstream-secret"),
            Arc::from("kvlt_test_gateway"),
            10.0,
            1.0,
        ));

        assert_eq!(
            post_evaluation_test(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let mut test_headers = HeaderMap::new();
        test_headers.insert(
            "x-kilovolt-evaluation-key",
            HeaderValue::from_static("kvlt_test_gateway"),
        );
        let response = post_evaluation_test(State(state.clone()), test_headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_text = String::from_utf8_lossy(&body);
        assert!(!body_text.contains("sk-provider-upstream-secret"));
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["model"], "gpt-4o-mini-2024-07-18");
        assert_eq!(payload["tokens"], 12);
        assert_eq!(payload["input_tokens"], 8);
        assert_eq!(payload["output_tokens"], 4);
        assert_eq!(payload["output_text"], "Kilovolt is working.");
        assert_eq!(payload["project_budget_usd"], 10.0);
        assert!(payload["spend_usd"].as_f64().unwrap() > 0.0);
        assert!(payload["project_calculated_spend_usd"].as_f64().unwrap() > 0.0);
        assert!(payload["project_remaining_usd"].as_f64().unwrap() < 10.0);
        assert!(state.evaluation_test_succeeded());
        assert!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.user_id == "kilovolt-evaluation")
        );
        assert_eq!(
            authorization_receiver.recv().await.unwrap(),
            "Bearer sk-provider-upstream-secret"
        );

        let onboarding = get_root(State(state)).await;
        let onboarding_body = onboarding.into_body().collect().await.unwrap().to_bytes();
        let onboarding_html = String::from_utf8_lossy(&onboarding_body);
        assert!(onboarding_html.contains("let verificationComplete = true;"));
        assert!(onboarding_html.contains("Kilovolt is working"));
        assert!(onboarding_html.contains("8 / 4 tokens"));
        assert!(onboarding_html.contains("Request succeeded"));
        server.abort();
    }

    #[tokio::test]
    async fn evaluation_test_failure_is_actionable_and_sanitized() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {"message": "rejected sk-provider-must-stay-secret"}
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut state = test_evaluation_state(0);
        state.openai_upstream_url = format!("http://{address}/v1/chat/completions");
        assert!(state.complete_evaluation_setup(
            Arc::from("sk-provider-must-stay-secret"),
            Arc::from("kvlt_failure_gateway"),
            10.0,
            1.0,
        ));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-kilovolt-evaluation-key",
            HeaderValue::from_static("kvlt_failure_gateway"),
        );

        let response = post_evaluation_test(State(state), headers).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("OpenAI rejected the saved API key"));
        assert!(!text.contains("sk-provider-must-stay-secret"));
        assert!(!text.contains("rejected sk-provider"));
        server.abort();
    }
}
