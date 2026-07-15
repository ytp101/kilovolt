'use client';

import React, { useState, useEffect } from 'react';
import { useRouter } from 'next/navigation';

interface TelemetryLog {
    timestamp: string;
    type: string;
    client_hash?: string;
    version: string;
    isDocker?: boolean;
    os?: string;
    arch?: string;
    ip: string;
    total_requests?: number;
    total_tokens?: number;
    total_users?: number;
    model_distribution?: { [key: string]: number };
}

interface TelemetryAnalytics {
    total_spend_under_management: number;
    total_requests_managed: number;
    total_tokens_managed: number;
    active_instances: string[];
}

export default function TelemetryDashboard({ 
    initialLogs, 
    initialAnalytics,
    initialStorageType
}: { 
    initialLogs: TelemetryLog[];
    initialAnalytics: TelemetryAnalytics;
    initialStorageType: string;
}) {
    const [logs, setLogs] = useState<TelemetryLog[]>(initialLogs);
    const [analytics, setAnalytics] = useState<TelemetryAnalytics>(initialAnalytics);
    const [storageType, setStorageType] = useState(initialStorageType);
    const [polling, setPolling] = useState(true);
    const router = useRouter();

    const fetchTelemetry = async () => {
        try {
            const res = await fetch('/api/telemetry-data');
            if (res.status === 401) {
                router.push('/login');
                return;
            }
            if (res.ok) {
                const data = await res.json();
                setLogs(data.logs);
                setAnalytics(data.analytics);
                setStorageType(data.storage_type);
            }
        } catch (e) {
            console.error('Failed to poll telemetry logs:', e);
        }
    };

    useEffect(() => {
        if (!polling) return;
        const interval = setInterval(fetchTelemetry, 5000);
        return () => clearInterval(interval);
    }, [polling]);

    // Handle logout
    const handleLogout = async () => {
        document.cookie = "admin_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:01 GMT;";
        router.push('/login');
        router.refresh();
    };

    // Calculate distributions from Logs
    const totalHandshakes = logs.length;
    const dockerCount = logs.filter(l => l.isDocker === true || l.isDocker === undefined).length; // Fallback
    const nativeCount = totalHandshakes - logs.filter(l => l.isDocker === true).length;

    const getDistribution = (key: 'os' | 'arch' | 'version') => {
        const counts: { [key: string]: number } = {};
        logs.forEach(l => {
            const val = l[key] || 'unknown';
            counts[val] = (counts[val] || 0) + 1;
        });
        return Object.entries(counts).sort((a, b) => b[1] - a[1]);
    };

    const osDistribution = getDistribution('os');
    const archDistribution = getDistribution('arch');
    const versionDistribution = getDistribution('version');

    // Format TSUM cost helper
    const formatCost = (val: number) => {
        if (val === 0) return '$0.00';
        if (val < 0.001) return `$${val.toFixed(7)}`;
        return `$${val.toFixed(4)}`;
    };

    return (
        <div className="min-h-screen bg-slate-950 text-slate-100 font-sans pb-20 relative overflow-hidden">
            {/* Background radial glow */}
            <div className="absolute top-0 left-1/2 -translate-x-1/2 w-[1200px] h-[400px] bg-yellow-500/5 blur-[120px] pointer-events-none rounded-full" />

            {/* Header */}
            <header className="border-b border-slate-900 bg-slate-950/50 backdrop-blur-md sticky top-0 z-50">
                <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
                    <div className="flex items-center space-x-3">
                        <span className="text-xl">📊</span>
                        <span className="text-lg font-black tracking-wider text-slate-100 uppercase">
                            Telemetry Analytics Hub
                        </span>
                    </div>
                    <div className="flex items-center space-x-4">
                        <button
                            onClick={() => setPolling(!polling)}
                            className={`text-xs px-3 py-1.5 rounded-full font-mono flex items-center space-x-2 transition ${
                                polling 
                                    ? 'bg-green-500/10 border border-green-500/25 text-green-400' 
                                    : 'bg-slate-900 border border-slate-800 text-slate-400'
                            }`}
                        >
                            <span className={`h-1.5 w-1.5 rounded-full ${polling ? 'bg-green-400 animate-pulse' : 'bg-slate-400'}`}></span>
                            <span>{polling ? 'Live Polling: 5s' : 'Polling Off'}</span>
                        </button>
                        <span className="text-xs bg-slate-900 border border-slate-800 text-slate-400 px-3 py-1.5 rounded-xl font-mono hidden sm:inline-block">
                            Storage: {storageType}
                        </span>
                        <button
                            onClick={handleLogout}
                            className="text-xs bg-red-500/10 border border-red-500/20 text-red-400 px-3 py-1.5 rounded-xl hover:bg-red-500/20 transition font-medium"
                        >
                            Logout
                        </button>
                    </div>
                </div>
            </header>

            {/* Content Container */}
            <main className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-10 space-y-10 relative z-10">
                
                {/* Stats Summary Grid */}
                <div className="grid grid-cols-1 md:grid-cols-5 gap-6">
                    {/* Card: TSUM */}
                    <div className="bg-slate-900/60 border border-yellow-500/10 rounded-2xl p-6 shadow-xl relative overflow-hidden group hover:border-yellow-500/30 transition duration-300">
                        <div className="absolute top-0 right-0 w-[100px] h-[100px] bg-yellow-500/5 blur-[30px] rounded-full pointer-events-none" />
                        <p className="text-xs text-yellow-500 font-bold uppercase tracking-wider">Total Spend Under Management (TSUM)</p>
                        <p className="text-3xl font-black text-yellow-400 mt-2 font-mono">
                            {formatCost(analytics.total_spend_under_management)}
                        </p>
                    </div>

                    {/* Card: Total Requests managed */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Total Requests Managed</p>
                        <p className="text-3xl font-black text-slate-100 mt-2 font-mono">
                            {analytics.total_requests_managed.toLocaleString()}
                        </p>
                    </div>

                    {/* Card: Total Tokens managed */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Tokens Transited (Aggregated)</p>
                        <p className="text-3xl font-black text-slate-100 mt-2 font-mono">
                            {analytics.total_tokens_managed.toLocaleString()}
                        </p>
                    </div>

                    {/* Card: Active Instances */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Active Instances</p>
                        <p className="text-3xl font-black text-slate-100 mt-2 font-mono">
                            {analytics.active_instances.length}
                        </p>
                    </div>

                    {/* Card: Docker vs Native */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Container Environments</p>
                        <div className="flex items-end justify-between mt-2">
                            <p className="text-3xl font-black text-slate-100 font-mono">
                                {totalHandshakes > 0 ? ((logs.filter(l => l.isDocker === true).length / totalHandshakes) * 100).toFixed(0) : 0}%
                            </p>
                            <span className="text-xs text-slate-500 font-mono pb-1">
                                {logs.filter(l => l.isDocker === true).length} Docker / {logs.filter(l => l.isDocker === false).length} Host
                            </span>
                        </div>
                    </div>
                </div>

                {/* Analytical Distributions */}
                <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                    {/* OS Platform Distributions */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl space-y-4">
                        <h3 className="text-sm font-bold text-slate-400 uppercase tracking-wider border-b border-slate-900 pb-2">
                            OS Platform Share
                        </h3>
                        <div className="space-y-2 font-mono text-sm">
                            {osDistribution.length === 0 ? (
                                <p className="text-slate-500 italic text-xs">No records</p>
                            ) : (
                                osDistribution.map(([os, count]) => {
                                    const pct = ((count / totalHandshakes) * 100).toFixed(1);
                                    return (
                                        <div key={os} className="space-y-1">
                                            <div className="flex justify-between text-slate-200">
                                                <span className="capitalize">{os}</span>
                                                <span>{pct}% ({count})</span>
                                            </div>
                                            <div className="w-full bg-slate-950 h-1.5 rounded-full overflow-hidden">
                                                <div className="bg-yellow-500 h-full" style={{ width: `${pct}%` }}></div>
                                            </div>
                                        </div>
                                    );
                                })
                            )}
                        </div>
                    </div>

                    {/* Hardware Architecture */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl space-y-4">
                        <h3 className="text-sm font-bold text-slate-400 uppercase tracking-wider border-b border-slate-900 pb-2">
                            Hardware Architecture
                        </h3>
                        <div className="space-y-2 font-mono text-sm">
                            {archDistribution.length === 0 ? (
                                <p className="text-slate-500 italic text-xs">No records</p>
                            ) : (
                                archDistribution.map(([arch, count]) => {
                                    const pct = ((count / totalHandshakes) * 100).toFixed(1);
                                    return (
                                        <div key={arch} className="space-y-1">
                                            <div className="flex justify-between text-slate-200">
                                                <span>{arch}</span>
                                                <span>{pct}% ({count})</span>
                                            </div>
                                            <div className="w-full bg-slate-950 h-1.5 rounded-full overflow-hidden">
                                                <div className="bg-amber-500 h-full" style={{ width: `${pct}%` }}></div>
                                            </div>
                                        </div>
                                    );
                                })
                            )}
                        </div>
                    </div>
                </div>

                {/* Handshake Logs Table */}
                <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                    <h3 className="text-sm font-bold text-slate-400 uppercase tracking-wider mb-4 border-b border-slate-900 pb-2">
                        Recent Telemetry Events (Startup & MAPD Handshakes)
                    </h3>
                    <div className="overflow-x-auto">
                        <table className="min-w-full divide-y divide-slate-900 text-sm">
                            <thead>
                                <tr className="text-slate-500 text-left font-medium">
                                    <th className="py-3 px-4">Timestamp</th>
                                    <th className="py-3 px-4">Client IP</th>
                                    <th className="py-3 px-4">Type</th>
                                    <th className="py-3 px-4">Client ID</th>
                                    <th className="py-3 px-4">Details / Metrics</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-slate-900/40 font-mono text-slate-300">
                                {logs.length === 0 ? (
                                    <tr>
                                        <td colSpan={5} className="py-6 text-center text-slate-500 italic">
                                            No telemetry checks transited yet.
                                        </td>
                                    </tr>
                                ) : (
                                    [...logs].reverse().map((log, idx) => {
                                        const shortHash = log.client_hash ? `${log.client_hash.slice(0, 8)}...` : 'n/a';
                                        
                                        // Dynamic details column based on telemetry type
                                        let details = '';
                                        if (log.type === 'startup') {
                                            const env = log.isDocker ? 'Docker' : 'Host';
                                            details = `Startup: Env: ${env} | OS: ${log.os} | Arch: ${log.arch} | Version: v${log.version}`;
                                        } else if (log.type === 'daily_mapd') {
                                            details = `24h Ping: Reqs: ${log.total_requests} | Tokens: ${log.total_tokens} | Users: ${log.total_users}`;
                                        } else {
                                            details = `Check-in: Version: v${log.version}`;
                                        }

                                        return (
                                            <tr key={idx} className="hover:bg-slate-900/20 transition">
                                                <td className="py-3 px-4 text-slate-500 text-xs">
                                                    {new Date(log.timestamp).toLocaleString()}
                                                </td>
                                                <td className="py-3 px-4 text-slate-400">{log.ip}</td>
                                                <td className="py-3 px-4">
                                                    <span className={`px-2 py-0.5 rounded text-xs font-bold ${
                                                        log.type === 'startup' 
                                                            ? 'text-yellow-400 bg-yellow-500/10 border border-yellow-500/20' 
                                                            : log.type === 'daily_mapd'
                                                            ? 'text-emerald-400 bg-emerald-500/10 border border-emerald-500/20'
                                                            : 'text-slate-400 bg-slate-950 border border-slate-850'
                                                    }`}>
                                                        {log.type}
                                                    </span>
                                                </td>
                                                <td className="py-3 px-4 text-slate-400 text-xs" title={log.client_hash}>
                                                    {shortHash}
                                                </td>
                                                <td className="py-3 px-4 text-slate-300 text-xs truncate max-w-xs" title={details}>
                                                    {details}
                                                </td>
                                            </tr>
                                        );
                                    })
                                )}
                            </tbody>
                        </table>
                    </div>
                </div>

            </main>
        </div>
    );
}
