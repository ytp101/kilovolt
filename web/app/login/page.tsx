'use client';

import React, { useState } from 'react';
import { useRouter } from 'next/navigation';

export default function LoginPage() {
    const [password, setPassword] = useState('');
    const [error, setError] = useState('');
    const [loading, setLoading] = useState(false);
    const router = useRouter();

    const handleSubmit = async (e: React.FormEvent) => {
        e.preventDefault();
        setError('');
        setLoading(true);

        try {
            const res = await fetch('/api/login', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ password })
            });

            const data = await res.json();

            if (res.ok && data.success) {
                router.push('/telemetry-dashboard');
                router.refresh();
            } else {
                setError(data.error || 'Login failed');
            }
        } catch (err) {
            setError('Could not connect to authentication server');
        } finally {
            setLoading(false);
        }
    };

    return (
        <div className="min-h-screen bg-slate-950 flex items-center justify-center font-sans px-4 relative overflow-hidden">
            {/* Background radial glow */}
            <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[600px] h-[600px] bg-yellow-500/5 blur-[120px] pointer-events-none rounded-full" />

            <div className="max-w-md w-full bg-slate-900/40 border border-slate-900 rounded-2xl p-8 backdrop-blur-md shadow-2xl relative z-10 space-y-8">
                <div className="text-center space-y-2">
                    <span className="text-4xl block animate-bounce">⚡</span>
                    <h2 className="text-2xl font-black tracking-wider text-slate-100 uppercase">
                        Admin Portal
                    </h2>
                    <p className="text-sm text-slate-500 font-medium">
                        Enter password to access telemetry diagnostics.
                    </p>
                </div>

                <form onSubmit={handleSubmit} className="space-y-6">
                    <div className="space-y-2">
                        <label className="text-xs font-bold text-slate-400 uppercase tracking-wider">
                            Password
                        </label>
                        <input
                            type="password"
                            required
                            value={password}
                            onChange={(e) => setPassword(e.target.value)}
                            placeholder="••••••••"
                            className="w-full bg-slate-950 border border-slate-850 hover:border-slate-800 focus:border-yellow-500 text-slate-100 rounded-xl px-4 py-3 text-sm focus:outline-none transition duration-300 font-mono"
                        />
                    </div>

                    {error && (
                        <div className="bg-red-500/10 border border-red-500/20 text-red-400 text-xs px-4 py-3 rounded-xl flex items-center space-x-2">
                            <span>⚠️</span>
                            <span>{error}</span>
                        </div>
                    )}

                    <button
                        type="submit"
                        disabled={loading}
                        className="w-full py-4 bg-gradient-to-r from-yellow-500 to-amber-500 text-slate-950 font-bold rounded-xl shadow-lg shadow-yellow-500/10 hover:from-yellow-400 hover:to-amber-400 hover:shadow-yellow-500/20 transition disabled:opacity-50 disabled:cursor-not-allowed text-sm uppercase tracking-wider"
                    >
                        {loading ? 'Verifying...' : 'Access Dashboard'}
                    </button>
                </form>
            </div>
        </div>
    );
}
