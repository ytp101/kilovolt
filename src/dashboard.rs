use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use crate::config::{AppState, RecentRequest};

// Struct for dashboard and stats API payloads
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
pub async fn get_stats(State(state): State<AppState>) -> Response {
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
pub async fn get_dashboard() -> impl IntoResponse {
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
                document.getElementById('default-budget').innerText = formatCost(data.budget.default_budget_usd);

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
                                <td class="py-3 px-4 text-slate-400">${req.timestamp}</td>
                                <td class="py-3 px-4 font-bold text-slate-300">${req.user_id}</td>
                                <td class="py-3 px-4 text-slate-400">${req.model}</td>
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
