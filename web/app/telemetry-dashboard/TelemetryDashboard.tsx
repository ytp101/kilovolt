'use client';

import React, { useState, useEffect } from 'react';
import { useRouter } from 'next/navigation';

interface TelemetryLog {
    timestamp: string;
    version: string;
    isDocker: boolean;
    os: string;
    arch: string;
    ip: string;
}

export default function TelemetryDashboard({ initialLogs }: { initialLogs: TelemetryLog[] }) {
    const [logs, setLogs] = useState<TelemetryLog[]>(initialLogs);
    const [polling, setPolling] = useState(true);
    const router = useRouter();

    const fetchLogs = async () => {
        try {
            const res = await fetch('/api/telemetry-data');
            if (res.status === 401) {
                router.push('/login');
                return;
            }
            if (res.ok) {
                const data = await res.json();
                setLogs(data);
            }
        } catch (e) {
            console.error('Failed to poll telemetry logs:', e);
        }
    };

    useEffect(() => {
        if (!polling) return;
        const interval = setInterval(fetchLogs, 5000);
        return () => clearInterval(interval);
    }, [polling]);

    // Handle logout
    const handleLogout = async () => {
        // Clear cookie client side by setting past expiry date
        document.cookie = "admin_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:01 GMT;";
        router.push('/login');
        router.refresh();
    };

    // Calculate Analytics
    const totalHandshakes = logs.length;
    const dockerCount = logs.filter(l => l.isDocker).length;
    const nativeCount = totalHandshakes - dockerCount;
    const dockerPercentage = totalHandshakes > 0 ? ((dockerCount / totalHandshakes) * 100).toFixed(1) : '0';

    // Grouping helper
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
                            Telemetry Analytics
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
                <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
                    {/* Card: Total Handshakes */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Total Update Checks</p>
                        <p className="text-4xl font-black text-slate-100 mt-2 font-mono">{totalHandshakes}</p>
                    </div>

                    {/* Card: Docker Environment Share */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider">Docker vs. Native</p>
                        <div className="flex items-end justify-between mt-2">
                            <p className="text-4xl font-black text-slate-100 font-mono">{dockerPercentage}%</p>
                            <span className="text-xs text-slate-400 pb-1 font-mono">
                                {dockerCount} Container / {nativeCount} Native
                            </span>
                        </div>
                    </div>

                    {/* Card: Version Distribution */}
                    <div className="bg-slate-900/40 border border-slate-900 rounded-2xl p-6 shadow-xl">
                        <p className="text-xs text-slate-500 font-bold uppercase tracking-wider mb-2">Versions</p>
                        <div className="space-y-1 text-sm max-h-12 overflow-y-auto font-mono">
                            {versionDistribution.length === 0 ? (
                                <p className="text-slate-500 italic text-xs">No entries</p>
                            ) : (
                                versionDistribution.map(([version, count]) => (
                                    <div key={version} className="flex justify-between text-slate-300">
                                        <span>v{version}</span>
                                        <span className="text-slate-500">{count} pings</span>
                                    </div>
                                ))
                            )}
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

                    {/* Architecture Share */}
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
                        Recent Handshake Records (Incognito Logs)
                    </h3>
                    <div className="overflow-x-auto">
                        <table className="min-w-full divide-y divide-slate-900 text-sm">
                            <thead>
                                <tr className="text-slate-500 text-left font-medium">
                                    <th className="py-3 px-4">Timestamp</th>
                                    <th className="py-3 px-4">Client IP</th>
                                    <th className="py-3 px-4">Version</th>
                                    <th className="py-3 px-4">Docker</th>
                                    <th className="py-3 px-4">OS</th>
                                    <th className="py-3 px-4">Architecture</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-slate-900/40 font-mono text-slate-300">
                                {logs.length === 0 ? (
                                    <tr>
                                        <td colSpan={6} className="py-6 text-center text-slate-500 italic">
                                            No pings captured yet. Start a Kilovolt engine to trigger.
                                        </td>
                                    </tr>
                                ) : (
                                    [...logs].reverse().map((log, idx) => (
                                        <tr key={idx} className="hover:bg-slate-900/20 transition">
                                            <td className="py-3 px-4 text-slate-500 text-xs">
                                                {new Date(log.timestamp).toLocaleString()}
                                            </td>
                                            <td className="py-3 px-4 text-slate-400">{log.ip}</td>
                                            <td className="py-3 px-4 font-bold text-slate-200">v{log.version}</td>
                                            <td className="py-3 px-4">
                                                <span className={`px-2 py-0.5 rounded text-xs font-bold ${
                                                    log.isDocker 
                                                        ? 'text-yellow-400 bg-yellow-500/10 border border-yellow-500/20' 
                                                        : 'text-slate-500 bg-slate-950 border border-slate-850'
                                                }`}>
                                                    {log.isDocker ? 'Docker' : 'Native'}
                                                </span>
                                            </td>
                                            <td className="py-3 px-4 capitalize text-slate-400">{log.os}</td>
                                            <td className="py-3 px-4 text-slate-400">{log.arch}</td>
                                        </tr>
                                    ))
                                )}
                            </tbody>
                        </table>
                    </div>
                </div>

            </main>
        </div>
    );
}
