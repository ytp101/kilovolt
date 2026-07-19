'use client';

import React, { useState } from 'react';

export default function Home() {
  const [email, setEmail] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState<string | null>(null);

  const handleJoinWaitlist = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    setError(null);

    try {
      const res = await fetch('/api/waitlist', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ email }),
      });

      const data = await res.json();
      if (res.ok) {
        setSubmitted(true);
        setMessage(data.message || 'Successfully joined the waitlist!');
      } else {
        setError(data.error || 'Failed to join waitlist. Please try again.');
      }
    } catch (err) {
      setError('An unexpected error occurred. Please try again.');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100 font-sans selection:bg-yellow-500 selection:text-slate-950 overflow-hidden relative">
      {/* Background radial glow */}
      <div className="absolute top-0 left-1/2 -translate-x-1/2 w-[1000px] h-[500px] bg-gradient-to-b from-yellow-500/10 to-transparent blur-[120px] pointer-events-none rounded-full" />

      {/* Header */}
      <header className="border-b border-slate-900 bg-slate-950/50 backdrop-blur-md sticky top-0 z-50">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
          <div className="flex items-center space-x-3 group">
            <span className="text-2xl transform group-hover:scale-125 transition duration-300">⚡</span>
            <span className="text-xl font-black tracking-wider bg-gradient-to-r from-yellow-400 to-amber-500 bg-clip-text text-transparent">
              KILOVOLT
            </span>
          </div>
          <div className="flex items-center space-x-6">
            <a 
              href="https://github.com/ytp101/kilovolt" 
              target="_blank" 
              rel="noreferrer" 
              className="text-sm font-medium text-slate-400 hover:text-yellow-400 transition"
            >
              GitHub
            </a>
            <a 
              href="/v1/update-check" 
              className="text-xs bg-slate-900 border border-slate-800 text-slate-300 px-3 py-1.5 rounded-full hover:border-yellow-500/30 transition font-mono flex items-center space-x-2"
            >
              <span className="h-1.5 w-1.5 rounded-full bg-green-400 animate-pulse"></span>
              <span>Telemetry: v1.3.1</span>
            </a>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-20 space-y-32 relative z-10">
        
        {/* Hero Section */}
        <section className="text-center max-w-4xl mx-auto space-y-8">
          <div className="inline-flex items-center space-x-2 px-3 py-1 bg-yellow-500/10 border border-yellow-500/25 rounded-full text-xs font-semibold text-yellow-400 tracking-wide uppercase">
            <span>🛡️ Bankruptcy Shield Active</span>
          </div>
          <h1 className="text-5xl sm:text-7xl font-extrabold tracking-tight text-white leading-tight">
            Stop Overdrafts on <br />
            <span className="bg-gradient-to-r from-yellow-400 via-amber-400 to-orange-500 bg-clip-text text-transparent">
              LLM API Streams
            </span>
          </h1>
          <p className="text-lg sm:text-xl text-slate-400 leading-relaxed max-w-2xl mx-auto">
            Kilovolt is an ultra-fast, zero-config reverse proxy that intercepts, tokenizes, and terminates AI streaming queries the millisecond they cross your budget.
          </p>

          {/* Waitlist Form Section */}
          <div className="max-w-md mx-auto space-y-4">
            {!submitted ? (
              <form onSubmit={handleJoinWaitlist} className="flex flex-col sm:flex-row items-center gap-3">
                <input
                  type="email"
                  placeholder="Enter your work email"
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  disabled={submitting}
                  className="w-full px-4 py-3 bg-slate-900/60 border border-slate-800 rounded-xl focus:outline-none focus:border-yellow-500 text-slate-100 placeholder:text-slate-500 font-mono text-sm transition"
                />
                <button
                  type="submit"
                  disabled={submitting}
                  className="w-full sm:w-auto px-6 py-3 bg-gradient-to-r from-yellow-500 to-amber-500 hover:from-yellow-400 hover:to-amber-400 text-slate-950 font-bold rounded-xl transition duration-200 transform active:scale-95 disabled:opacity-50 text-sm whitespace-nowrap cursor-pointer shadow-lg shadow-yellow-500/10"
                >
                  {submitting ? 'Joining...' : 'Join Waitlist'}
                </button>
              </form>
            ) : (
              <div className="p-4 bg-emerald-500/10 border border-emerald-500/25 rounded-xl text-emerald-400 text-sm font-medium">
                🎉 {message}
              </div>
            )}
            {error && (
              <p className="text-red-400 text-xs font-mono">{error}</p>
            )}
            <div className="pt-2">
              <a 
                href="https://github.com/ytp101/kilovolt#readme"
                target="_blank"
                rel="noreferrer"
                className="text-xs text-slate-400 hover:text-yellow-400 underline decoration-slate-800 hover:decoration-yellow-500 transition font-medium"
              >
                Or read the self-hosted setup docs &rarr;
              </a>
            </div>
          </div>
        </section>

        {/* Feature Grid */}
        <section className="grid grid-cols-1 md:grid-cols-3 gap-8">
          {/* Card 1 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">🛡️</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Bankruptcy Shield</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Charges prompt tokens upfront. Decodes and audits stream chunks on the fly and sever sockets immediately when spending bounds are breached.
              </p>
            </div>
          </div>

          {/* Card 2 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">♊</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Gemini SSE Translation</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Seamlessly routes requests to Google Gemini, translating camelCase stream outputs to OpenAI-compatible choices delta packets dynamically.
              </p>
            </div>
          </div>

          {/* Card 3 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">⚡</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Zero-Copy Piping</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Pipes byte-streams asynchronously with O(1) space complexity, maintaining microsecond-level proxy overhead and minimal memory footprints.
              </p>
            </div>
          </div>
        </section>

        {/* Code Terminal Simulation */}
        <section className="bg-slate-950/80 border border-slate-900 rounded-2xl p-6 shadow-2xl max-w-3xl mx-auto space-y-4 font-mono text-xs sm:text-sm">
          <div className="flex items-center space-x-2 border-b border-slate-900 pb-3 mb-4">
            <span className="h-3 w-3 rounded-full bg-red-500"></span>
            <span className="h-3 w-3 rounded-full bg-yellow-500"></span>
            <span className="h-3 w-3 rounded-full bg-green-500"></span>
            <span className="text-slate-500 ml-2">curl-stream-demo.sh</span>
          </div>
          <div className="text-slate-400 space-y-2">
            <p className="text-slate-500"># Point your client directly to local Kilovolt proxy gateway</p>
            <p>
              <span className="text-yellow-500">curl</span> -i -N -X POST http://127.0.0.1:8080/v1/chat/completions \
            </p>
            <p className="pl-4">
              -H <span className="text-emerald-400">"Authorization: Bearer sk-proj-your-key"</span> \
            </p>
            <p className="pl-4">
              -H <span className="text-emerald-400">"X-User-ID: developer_alice"</span> \
            </p>
            <p className="pl-4">
              -d <span className="text-emerald-400">{`'{"model": "gemini-1.5-flash", "messages": [{"role": "user", "content": "Hi!"}], "stream": true}'`}</span>
            </p>
          </div>
        </section>

      </main>

      {/* Footer */}
      <footer className="border-t border-slate-900 bg-slate-950/80 py-10 mt-32 text-center text-xs text-slate-600 relative z-10">
        <p>Kilovolt Telemetry Hub & Landing Server &copy; 2026. Powered by Next.js App Router.</p>
      </footer>
    </div>
  );
}
