'use client';

import React, { useState } from 'react';

const DOCKER_COMMAND = 'docker run -p 127.0.0.1:8080:8080 yodsarun/kilovolt-proxy:latest';

export default function Home() {
  const [dockerCopied, setDockerCopied] = useState(false);

  const copyDockerCommand = async () => {
    await navigator.clipboard.writeText(DOCKER_COMMAND);
    setDockerCopied(true);
    window.setTimeout(() => setDockerCopied(false), 1800);
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
              <span>Version status</span>
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
          <div className="max-w-3xl mx-auto overflow-hidden rounded-2xl border border-yellow-500/30 bg-slate-900/70 text-left shadow-2xl shadow-yellow-500/5">
            <div className="flex items-center justify-between border-b border-slate-800 px-4 py-3 text-xs font-semibold uppercase tracking-wider text-slate-400">
              <span>Run with Docker</span>
              <span className="text-emerald-400">localhost only</span>
            </div>
            <div className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center">
              <code className="min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-sm text-slate-100">
                {DOCKER_COMMAND}
              </code>
              <button
                type="button"
                onClick={copyDockerCommand}
                className="shrink-0 rounded-lg border border-slate-700 bg-slate-800 px-4 py-2 text-sm font-bold text-slate-100 transition hover:border-yellow-500/50 hover:text-yellow-300 focus:outline-none focus:ring-2 focus:ring-yellow-400 cursor-pointer"
                aria-live="polite"
                aria-label={dockerCopied ? 'Docker command copied' : 'Copy Docker command'}
              >
                {dockerCopied ? 'Copied' : 'Copy'}
              </button>
            </div>
          </div>

          <div className="mx-auto max-w-2xl space-y-3">
            <p className="text-lg text-slate-300 sm:text-xl">
              Run Kilovolt locally, then open the URL printed in your terminal.
            </p>
            <p className="text-sm leading-relaxed text-slate-500">
              No Git clone, Cargo, Docker Compose, or <code className="text-slate-300">.env</code> is required to evaluate the gateway.
            </p>
            <a
              href="https://github.com/ytp101/kilovolt#readme"
              target="_blank"
              rel="noreferrer"
              className="inline-block text-sm font-medium text-slate-400 underline decoration-slate-700 transition hover:text-yellow-400 hover:decoration-yellow-500"
            >
              Read the self-hosted documentation &rarr;
            </a>
          </div>
        </section>

        {/* Feature Grid */}
        <section className="grid grid-cols-1 md:grid-cols-3 gap-8">
          {/* Card 1 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">🛡️</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Calculated Budget Circuit Breaker</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Atomically reserves estimated prompt cost against project and user limits, then stops before forwarding an output increment that would exceed either limit.
              </p>
            </div>
          </div>

          {/* Card 2 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">♊</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Gemini SSE Translation</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Experimental streaming-only translation for supported Gemini candidate text into OpenAI-shaped SSE chunks.
              </p>
            </div>
          </div>

          {/* Card 3 */}
          <div className="bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-8 shadow-xl backdrop-blur-sm transition duration-300 flex flex-col justify-between group">
            <div className="space-y-4">
              <div className="text-3xl">⚡</div>
              <h3 className="text-xl font-bold text-slate-200 group-hover:text-yellow-400 transition">Bounded SSE Processing</h3>
              <p className="text-sm text-slate-400 leading-relaxed">
                Reconstructs UTF-8 SSE events across arbitrary network chunks and enforces an explicit maximum buffered frame size.
              </p>
            </div>
          </div>
        </section>

        {/* Verified Local Benchmark Table */}
        <section className="max-w-4xl mx-auto space-y-6">
          <div className="text-center space-y-2">
            <h2 className="text-2xl sm:text-3xl font-extrabold text-white">
              Measured locally with <span className="bg-gradient-to-r from-yellow-400 to-amber-500 bg-clip-text text-transparent">reproducible evidence</span>
            </h2>
            <p className="text-sm text-slate-400 max-w-xl mx-auto">
              Apple M4, macOS arm64, release build, deterministic loopback mock. These are one-machine observations, not universal guarantees.
            </p>
          </div>
          <div className="bg-slate-900/30 border border-slate-900 rounded-2xl overflow-hidden shadow-2xl backdrop-blur-sm">
            <div className="overflow-x-auto">
              <table className="w-full text-left border-collapse text-xs sm:text-sm">
                <thead>
                  <tr className="border-b border-slate-900 bg-slate-950/40 text-slate-400 font-mono">
                    <th className="p-4 sm:p-5">Dimension</th>
                    <th className="p-4 sm:p-5">Direct Local Mock</th>
                    <th className="p-4 sm:p-5 text-yellow-400 font-bold">Through Kilovolt ⚡</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-900 text-slate-300 font-mono">
                  <tr>
                    <td className="p-4 sm:p-5 text-slate-400 font-sans font-medium">Idle RSS across 3 runs</td>
                    <td className="p-4 sm:p-5">—</td>
                    <td className="p-4 sm:p-5 text-emerald-400 font-bold">10,208–10,256 KiB</td>
                  </tr>
                  <tr>
                    <td className="p-4 sm:p-5 text-slate-400 font-sans font-medium">JSON total p50, concurrency 1</td>
                    <td className="p-4 sm:p-5">0.107–0.115 ms</td>
                    <td className="p-4 sm:p-5 text-emerald-400 font-bold">0.149–0.152 ms</td>
                  </tr>
                  <tr>
                    <td className="p-4 sm:p-5 text-slate-400 font-sans font-medium">Streaming TTFB p50, concurrency 100</td>
                    <td className="p-4 sm:p-5">2.977–3.186 ms</td>
                    <td className="p-4 sm:p-5 text-emerald-400 font-bold">2.743–3.528 ms</td>
                  </tr>
                  <tr>
                    <td className="p-4 sm:p-5 text-slate-400 font-sans font-medium">5,000-event stream total p50</td>
                    <td className="p-4 sm:p-5">2.484–2.663 ms</td>
                    <td className="p-4 sm:p-5 text-emerald-400 font-bold">21.233–21.371 ms</td>
                  </tr>
                  <tr>
                    <td className="p-4 sm:p-5 text-slate-400 font-sans font-medium">Normal-workload RSS after warm-up</td>
                    <td className="p-4 sm:p-5">—</td>
                    <td className="p-4 sm:p-5 text-emerald-400 font-bold">about 61.5–64.5 MiB</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
          <p className="text-center text-xs text-slate-500">
            Methodology, raw JSON, limitations, and the dirty source-state disclosure are in the repository benchmark documentation.
          </p>
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
              -H <span className="text-emerald-400">&quot;Authorization: Bearer sk-proj-your-key&quot;</span> \
            </p>
            <p className="pl-4">
              -H <span className="text-emerald-400">&quot;X-User-ID: developer_alice&quot;</span> \
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
