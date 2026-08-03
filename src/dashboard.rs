use crate::config::{AppState, RecentRequest, secrets_match};
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

#[derive(serde::Deserialize)]
pub struct SetupForm {
    provider_api_key: String,
    project_budget: String,
    default_budget: String,
}

// Struct for dashboard and stats API payloads
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
    project_calculated_spend_usd: f64,
    user_calculated_spend_usd: f64,
}

/// Helper function to retrieve RSS memory usage of the current process on Linux.
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
    // Fallback/mock RSS memory usage (e.g. 15MB) when running locally on macOS
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
            axum::http::HeaderValue::from_static("Basic realm=\"Kilovolt dashboard\""),
        );
        Some(response)
    }
}

/// REST endpoint `/api/stats` to expose local operational and budget state.
pub async fn get_stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = dashboard_auth_failure(&state, &headers) {
        return response;
    }

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
            uptime_seconds: uptime,
            memory_usage_kb: memory_usage,
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

fn setup_page(error: Option<&str>) -> Response {
    let error = error.map_or_else(String::new, |message| {
        format!(
            "<div class=\"error\" role=\"alert\">{}</div>",
            escape_html(message)
        )
    });
    let html = SETUP_HTML.replace("{{SETUP_ERROR}}", &error);
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

fn generate_gateway_key() -> String {
    format!(
        "kvlt_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn masked_provider_key(provider_api_key: &str) -> String {
    let suffix: String = provider_api_key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("••••{suffix}")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

fn render_dashboard(state: &AppState) -> String {
    let panel = state.evaluation_setup_snapshot().map_or_else(
        || {
            r#"<div class="rounded-xl border border-amber-700/60 bg-amber-950/30 px-4 py-3 text-sm text-amber-200">
                <strong>Process-local ledger:</strong>
                spend resets on restart and multiple instances do not share one budget.
            </div>"#
                .to_string()
        },
        |setup| {
            let gateway_key = escape_html(setup.gateway_key());
            let provider_key = escape_html(&masked_provider_key(setup.provider_api_key()));
            let integration_style = if state.evaluation_test_succeeded() {
                ""
            } else {
                "display: none"
            };
            let test_status = if state.evaluation_test_succeeded() {
                "Kilovolt is working. Now connect your application."
            } else {
                ""
            };
            format!(
                r#"<div class="rounded-xl border border-amber-600 bg-amber-950/50 px-5 py-4 text-sm text-amber-100">
                    <strong>Evaluation mode:</strong> configuration and calculated spend are stored only in the running Kilovolt process. Restarting, removing, or replacing the container resets setup and usage.
                    This localhost quick start is not production-secure.
                </div>
                <section class="bg-slate-900/60 border border-slate-800 rounded-2xl p-6 shadow-xl space-y-4">
                    <div>
                        <h2 class="text-lg font-semibold text-slate-100">Connect your trusted backend</h2>
                        <p class="mt-1 text-sm text-slate-400">Stored OpenAI key: <code>{provider_key}</code>. Your application uses the Kilovolt gateway key below; Kilovolt inserts the stored provider key upstream.</p>
                    </div>
                    <div class="rounded-lg border border-red-800/70 bg-red-950/30 px-4 py-3 text-sm text-red-200">
                        <strong>Trusted identity only:</strong> derive <code>X-User-ID</code> in an authenticated backend. Never let an untrusted browser or mobile client choose arbitrary user IDs.
                    </div>
                    <div>
                        <p class="mb-2 text-xs uppercase tracking-wide text-slate-500">Kilovolt gateway key</p>
                        <pre class="overflow-x-auto rounded-lg bg-slate-950 p-4 text-sm text-amber-300"><code id="gateway-key">{gateway_key}</code></pre>
                        <button id="copy-gateway-button" type="button" onclick="copyGatewayKey()" class="mt-2 rounded-lg border border-slate-700 bg-slate-800 px-3 py-2 text-sm font-semibold text-slate-200 hover:bg-slate-700">Copy gateway key</button>
                    </div>
                    <div class="rounded-xl border border-sky-800 bg-sky-950/30 p-4">
                        <h3 class="font-semibold text-sky-100">Verify the full path</h3>
                        <p class="mt-1 text-sm text-sky-200">This sends one real request to <code>gpt-4o-mini</code> with a 16-token output cap. It may incur a very small OpenAI charge.</p>
                        <button id="test-request-button" type="button" onclick="sendEvaluationTest()" class="mt-3 rounded-lg bg-sky-500 px-4 py-2 font-bold text-slate-950 hover:bg-sky-400">Send test request</button>
                        <p id="test-request-status" class="mt-3 text-sm text-slate-200" role="status">{test_status}</p>
                    </div>
                </section>
                <section id="integration-instructions" style="{integration_style}" class="bg-slate-900/60 border border-slate-800 rounded-2xl p-6 shadow-xl space-y-4">
                    <div>
                        <h2 class="text-lg font-semibold text-green-300">Kilovolt is working. Now connect your application.</h2>
                        <p class="mt-1 text-sm text-slate-400">Change the OpenAI-compatible base URL, use the generated Kilovolt key, and set the trusted user identity on each request.</p>
                    </div>
                    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
                        <div>
                            <p class="mb-2 text-sm font-semibold text-slate-300">Python</p>
                            <pre class="overflow-x-auto rounded-lg bg-slate-950 p-4 text-xs text-slate-300"><code>from openai import OpenAI

client = OpenAI(
    api_key="{gateway_key}",
    base_url="http://127.0.0.1:8080/v1",
)

response = client.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{{"role": "user", "content": "Hello"}}],
    max_completion_tokens=100,
    extra_headers={{
        "X-User-ID": authenticated_user_id,
    }},
)</code></pre>
                        </div>
                        <div>
                            <p class="mb-2 text-sm font-semibold text-slate-300">TypeScript</p>
                            <pre class="overflow-x-auto rounded-lg bg-slate-950 p-4 text-xs text-slate-300"><code>import OpenAI from "openai";

const client = new OpenAI({{
  apiKey: "{gateway_key}",
  baseURL: "http://127.0.0.1:8080/v1",
}});

await client.chat.completions.create(
  {{
    model: "gpt-4o-mini",
    messages: [{{ role: "user", content: "Hello" }}],
    max_completion_tokens: 100,
  }},
  {{ headers: {{ "X-User-ID": authenticatedUserId }} }},
);</code></pre>
                        </div>
                    </div>
                    <p class="text-sm text-slate-400">More detail: <a class="text-sky-400 underline" href="https://github.com/ytp101/kilovolt/blob/main/docs/configuration.md">configuration</a>, <a class="text-sky-400 underline" href="https://github.com/ytp101/kilovolt/blob/main/docs/cookbook/openai-python.md">Python integration</a>, and <a class="text-sky-400 underline" href="https://github.com/ytp101/kilovolt/blob/main/docs/cookbook/openai-node.md">TypeScript integration</a>.</p>
                </section>"#
            )
        },
    );
    DASHBOARD_HTML.replace("{{MODE_PANEL}}", &panel)
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
    let authorized = browser_headers
        .get("x-kilovolt-evaluation-key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|actual| secrets_match(setup.gateway_key(), actual));
    if !authorized {
        return with_no_store(
            (
                StatusCode::UNAUTHORIZED,
                Json(EvaluationTestPayload {
                    ok: false,
                    status: StatusCode::UNAUTHORIZED.as_u16(),
                    message: "Kilovolt gateway key authentication failed.".to_string(),
                    project_calculated_spend_usd: state
                        .budget_ledger
                        .project_snapshot()
                        .committed_spend,
                    user_calculated_spend_usd: state
                        .budget_ledger
                        .user_snapshot("kilovolt-evaluation")
                        .committed_spend,
                }),
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
    headers.insert("x-user-id", HeaderValue::from_static("kilovolt-evaluation"));
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
                    Json(EvaluationTestPayload {
                        ok: false,
                        status: StatusCode::BAD_GATEWAY.as_u16(),
                        message: "The test response could not be read safely.".to_string(),
                        project_calculated_spend_usd: state
                            .budget_ledger
                            .project_snapshot()
                            .committed_spend,
                        user_calculated_spend_usd: state
                            .budget_ledger
                            .user_snapshot("kilovolt-evaluation")
                            .committed_spend,
                    }),
                )
                    .into_response(),
            );
        }
    };

    let ok = status.is_success();
    if ok {
        state.mark_evaluation_test_succeeded();
    }
    let error_detail = serde_json::from_slice::<serde_json::Value>(&response_bytes)
        .ok()
        .and_then(|value| {
            let message = value.pointer("/error/message")?.as_str()?;
            let code = value
                .pointer("/error/code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("upstream_error");
            Some(format!("{code}: {message}"))
        })
        .unwrap_or_else(|| "The provider or Kilovolt rejected the request.".to_string())
        .replace(setup.provider_api_key(), "[redacted]");
    let message = if ok {
        "Kilovolt is working. Now connect your application.".to_string()
    } else {
        format!(
            "Test request failed with HTTP {}: {error_detail}",
            status.as_u16()
        )
    };
    let project_spend = state.budget_ledger.project_snapshot().committed_spend;
    let user_spend = state
        .budget_ledger
        .user_snapshot("kilovolt-evaluation")
        .committed_spend;

    with_no_store(
        (
            status,
            Json(EvaluationTestPayload {
                ok,
                status: status.as_u16(),
                message,
                project_calculated_spend_usd: project_spend,
                user_calculated_spend_usd: user_spend,
            }),
        )
            .into_response(),
    )
}

/// Root route for the Docker evaluation setup and dashboard journey.
pub async fn get_root(State(state): State<AppState>) -> Response {
    if !state.evaluation_mode() {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, "/dashboard")]).into_response();
    }
    if !state.evaluation_setup_complete() {
        return setup_page(None);
    }
    with_no_store(Html(render_dashboard(&state)).into_response())
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

    let gateway_key = generate_gateway_key();
    if !state.complete_evaluation_setup(
        Arc::from(provider_api_key),
        Arc::from(gateway_key),
        project_budget,
        default_budget,
    ) {
        return StatusCode::NOT_FOUND.into_response();
    }

    with_no_store(Html(render_dashboard(&state)).into_response())
}

/// Route handler to render the authenticated embedded HTML dashboard.
pub async fn get_dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if state.evaluation_mode() && !state.evaluation_setup_complete() {
        return setup_page(None);
    }
    if let Some(response) = dashboard_auth_failure(&state, &headers) {
        return response;
    }

    with_no_store(Html(render_dashboard(&state)).into_response())
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
        {{MODE_PANEL}}
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
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Project Spend / Limit</p>
                        <p class="text-xl font-bold text-slate-100 mt-1 font-mono"><span id="project-spend">-</span> / <span id="project-budget">-</span></p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Default User Limit</p>
                        <p id="default-budget" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Recent Accepted</p>
                        <p id="recent-accepted" class="text-xl font-bold text-green-400 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Recent Blocked</p>
                        <p id="recent-blocked" class="text-xl font-bold text-red-400 mt-1 font-mono">-</p>
                    </div>
                    <div class="bg-slate-950/60 p-4 rounded-xl border border-slate-850 col-span-2">
                        <p class="text-xs text-slate-500 font-medium uppercase tracking-wider">Total Accounted Tokens</p>
                        <p id="total-tokens" class="text-xl font-bold text-slate-100 mt-1 font-mono">-</p>
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
                            <th class="py-3 px-4">Request ID</th>
                            <th class="py-3 px-4">Time</th>
                            <th class="py-3 px-4">User ID</th>
                            <th class="py-3 px-4">Model</th>
                            <th class="py-3 px-4 text-right">Tokens</th>
                            <th class="py-3 px-4 text-right">Cost</th>
                            <th class="py-3 px-4">Status</th>
                            <th class="py-3 px-4 text-right">Latency</th>
                        </tr>
                    </thead>
                    <tbody id="recent-requests-table" class="divide-y divide-slate-800/60 text-slate-300 font-mono">
                        <tr>
                            <td colspan="8" class="py-4 text-center text-slate-500 italic">Waiting for traffic...</td>
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

        function formatCost(val) {
            if (val === 0) return '$0.00';
            if (val < 0.0001) return `$${val.toFixed(7)}`;
            return `$${val.toFixed(5)}`;
        }

        function escapeHtml(value) {
            return String(value)
                .replaceAll('&', '&amp;')
                .replaceAll('<', '&lt;')
                .replaceAll('>', '&gt;')
                .replaceAll('"', '&quot;')
                .replaceAll("'", '&#039;');
        }

        async function copyGatewayKey() {
            const key = document.getElementById('gateway-key');
            const button = document.getElementById('copy-gateway-button');
            if (!key || !button) return;
            await navigator.clipboard.writeText(key.textContent);
            button.innerText = 'Copied';
        }

        async function sendEvaluationTest() {
            const button = document.getElementById('test-request-button');
            const statusText = document.getElementById('test-request-status');
            if (!button || !statusText) return;
            button.disabled = true;
            statusText.innerText = 'Sending a small paid provider request...';
            try {
                const gatewayKey = document.getElementById('gateway-key').textContent;
                const response = await fetch('/evaluation/test', {
                    method: 'POST',
                    headers: { 'X-Kilovolt-Evaluation-Key': gatewayKey },
                });
                const result = await response.json();
                statusText.innerText = `${result.message} Project calculated spend: ${formatCost(result.project_calculated_spend_usd)}.`;
                if (result.ok) {
                    document.getElementById('integration-instructions').style.display = '';
                    await fetchStats();
                }
            } catch (err) {
                statusText.innerText = 'The test could not be completed. Check the Kilovolt container logs for a non-secret connection error.';
            } finally {
                button.disabled = false;
            }
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
                document.getElementById('project-spend').innerText = formatCost(data.budget.current_project_spend_usd);
                document.getElementById('project-budget').innerText = formatCost(data.budget.project_budget_usd);
                document.getElementById('default-budget').innerText = formatCost(data.budget.default_budget_usd);
                document.getElementById('recent-accepted').innerText = data.budget.recent_accepted_requests.toLocaleString();
                document.getElementById('recent-blocked').innerText = data.budget.recent_blocked_requests.toLocaleString();

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
                                <span class="font-medium text-slate-400">${escapeHtml(user)}</span>
                                <span class="${statusClass}">${formatCost(spend)}</span>
                            </div>
                        `;
                    });
                }

                // Render recent requests
                const tableBody = document.getElementById('recent-requests-table');
                tableBody.innerHTML = '';
                if (data.budget.recent_requests.length === 0) {
                    tableBody.innerHTML = '<tr><td colspan="8" class="py-4 text-center text-slate-500 italic">Waiting for traffic...</td></tr>';
                } else {
                    data.budget.recent_requests.forEach(req => {
                        const statusClass = req.status >= 400 ? 'text-red-400' : 'text-green-400';
                        const shortReqId = req.request_id ? `${req.request_id.slice(0, 8)}...` : 'n/a';
                        
                        tableBody.innerHTML += `
                            <tr class="hover:bg-slate-900/30 transition">
                                <td class="py-3 px-4 text-slate-500 font-mono">${shortReqId}</td>
                                <td class="py-3 px-4 text-slate-400">${escapeHtml(req.timestamp)}</td>
                                <td class="py-3 px-4 font-bold text-slate-300">${escapeHtml(req.user_id)}</td>
                                <td class="py-3 px-4 text-slate-400">${escapeHtml(req.model)}</td>
                                <td class="py-3 px-4 text-right text-slate-300">${req.tokens.toLocaleString()}</td>
                                <td class="py-3 px-4 text-right text-emerald-400 font-semibold">${formatCost(req.cost)}</td>
                                <td class="py-3 px-4"><span class="px-2 py-0.5 rounded text-xs font-bold ${statusClass} bg-slate-950 border border-slate-800">${req.status}</span></td>
                                <td class="py-3 px-4 text-right text-amber-500 font-semibold">${req.duration_ms} ms</td>
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

const SETUP_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Set up Kilovolt</title>
    <style>
        :root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, sans-serif; }
        * { box-sizing: border-box; }
        body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #020617; color: #e2e8f0; padding: 24px; }
        main { width: min(100%, 620px); background: #0f172a; border: 1px solid #334155; border-radius: 18px; padding: 32px; box-shadow: 0 24px 60px #0008; }
        h1 { margin: 0 0 8px; font-size: 30px; }
        .lead { color: #94a3b8; line-height: 1.55; margin: 0 0 24px; }
        .warning, .error { border-radius: 10px; padding: 12px 14px; margin: 0 0 20px; line-height: 1.45; }
        .warning { border: 1px solid #b45309; background: #451a0355; color: #fde68a; }
        .error { border: 1px solid #b91c1c; background: #450a0a88; color: #fecaca; }
        label { display: block; margin: 18px 0 7px; font-size: 14px; font-weight: 700; }
        input { width: 100%; border: 1px solid #475569; border-radius: 9px; padding: 12px 13px; background: #020617; color: #f8fafc; font: inherit; }
        input:focus { outline: 2px solid #facc15; outline-offset: 2px; }
        .hint { display: block; color: #64748b; font-size: 12px; margin-top: 6px; }
        button { width: 100%; margin-top: 24px; border: 0; border-radius: 9px; padding: 13px; background: #eab308; color: #1c1917; font-weight: 800; font-size: 15px; cursor: pointer; }
        button:hover { background: #facc15; }
        code { color: #fde047; }
    </style>
</head>
<body>
    <main>
        <h1>⚡ Set up Kilovolt</h1>
        <p class="lead">Add your OpenAI key and small evaluation budgets. Kilovolt will generate a separate high-entropy gateway key for your backend.</p>
        <div class="warning"><strong>Evaluation mode:</strong> configuration and calculated spend are stored only in the running Kilovolt process. Restarting, removing, or replacing the container resets setup and usage.</div>
        {{SETUP_ERROR}}
        <form action="/setup" method="post" autocomplete="off">
            <label for="provider-api-key">OpenAI API key</label>
            <input id="provider-api-key" name="provider_api_key" type="password" placeholder="sk-..." required autofocus spellcheck="false" autocomplete="off">
            <span class="hint">Stored only in this process's memory, masked after setup, and sent only to the upstream provider.</span>

            <label for="project-budget">Project calculated-spend limit (USD)</label>
            <input id="project-budget" name="project_budget" type="number" value="10" min="0" step="0.01" required>

            <label for="default-budget">Default per-user calculated-spend limit (USD)</label>
            <input id="default-budget" name="default_budget" type="number" value="1" min="0" step="0.01" required>
            <span class="hint"><code>X-User-ID</code> identifies each user for this limit and must be set by your trusted backend.</span>

            <button type="submit">Finish setup</button>
        </form>
    </main>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::{
        SetupForm, generate_gateway_key, get_dashboard, get_root, get_stats, post_evaluation_test,
        post_setup,
    };
    use axum::extract::{Form, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::routing::post;
    use axum::{Json, Router};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use http_body_util::BodyExt;
    use std::sync::Arc;

    use crate::config::{test_evaluation_state, test_state};

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

        let dashboard = get_dashboard(State(state), headers).await;
        assert_eq!(dashboard.status(), StatusCode::OK);
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
    async fn evaluation_setup_validates_input_masks_provider_and_closes_after_success() {
        let state = test_evaluation_state(0);
        let root = get_root(State(state.clone())).await;
        let root_body = root.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&root_body).contains("Finish setup"));

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
        assert_eq!(configured.status(), StatusCode::OK);
        assert_eq!(
            configured
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let configured_body = configured.into_body().collect().await.unwrap().to_bytes();
        let configured_html = String::from_utf8_lossy(&configured_body);
        assert!(!configured_html.contains(provider_key));
        assert!(configured_html.contains("••••ABCD"));
        assert!(configured_html.contains("Send test request"));

        let setup = state.evaluation_setup_snapshot().unwrap();
        assert!(setup.gateway_key().starts_with("kvlt_"));
        assert_eq!(setup.gateway_key().len(), 69);
        assert_eq!(state.effective_budgets(), (7.5, 0.75));
        assert_eq!(
            get_stats(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::OK
        );

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
    async fn evaluation_test_uses_provider_key_and_records_calculated_spend() {
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
        assert!(payload["project_calculated_spend_usd"].as_f64().unwrap() > 0.0);
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
        server.abort();
    }
}
