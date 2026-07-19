import React from 'react';
import { getAllPosts } from './utils';

export const metadata = {
  title: 'Kilovolt Engineering Blog — Architectural Guides & Post-Mortems',
  description: 'Deep-dive technical guides on scaling LLM pipelines, preventing OOM memory crashes, and stopping runaway AI api token billing using Rust.',
};

export default async function BlogIndex() {
  const posts = await getAllPosts();

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100 font-sans selection:bg-yellow-500 selection:text-slate-950 overflow-hidden relative pb-20">
      {/* Background radial glow */}
      <div className="absolute top-0 left-1/2 -translate-x-1/2 w-[1000px] h-[500px] bg-gradient-to-b from-yellow-500/10 to-transparent blur-[120px] pointer-events-none rounded-full" />

      {/* Header */}
      <header className="border-b border-slate-900 bg-slate-950/50 backdrop-blur-md sticky top-0 z-50">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
          <a href="/" className="flex items-center space-x-3 group">
            <span className="text-2xl transform group-hover:scale-125 transition duration-300">⚡</span>
            <span className="text-xl font-black tracking-wider bg-gradient-to-r from-yellow-400 to-amber-500 bg-clip-text text-transparent">
              KILOVOLT
            </span>
          </a>
          <div className="flex items-center space-x-6">
            <a 
              href="https://github.com/ytp101/kilovolt" 
              target="_blank" 
              rel="noreferrer" 
              className="text-sm font-medium text-slate-400 hover:text-yellow-400 transition"
            >
              GitHub
            </a>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-20 space-y-16 relative z-10">
        
        {/* Title */}
        <div className="space-y-4 text-center sm:text-left">
          <h1 className="text-4xl sm:text-5xl font-extrabold tracking-tight text-white leading-tight">
            Kilovolt <span className="bg-gradient-to-r from-yellow-400 to-amber-500 bg-clip-text text-transparent">Engineering Blog</span>
          </h1>
          <p className="text-slate-400 text-lg max-w-xl">
            Architectural guides, post-mortems, and technical walkthroughs for cost-efficient and memory-safe LLM production engineering.
          </p>
        </div>

        {/* Post List Grid */}
        <section className="space-y-6">
          {posts.map((post) => (
            <a 
              key={post.metadata.slug}
              href={`/blog/${post.metadata.slug}`}
              className="block bg-slate-900/40 border border-slate-900 hover:border-slate-800 rounded-2xl p-6 sm:p-8 shadow-xl backdrop-blur-sm transition duration-300 group"
            >
              <div className="space-y-4">
                <div className="flex items-center justify-between text-xs text-slate-500 font-mono">
                  <span>{post.metadata.date}</span>
                  <span className="text-yellow-500 group-hover:underline font-semibold">Read Article &rarr;</span>
                </div>
                <h3 className="text-xl sm:text-2xl font-bold text-slate-200 group-hover:text-yellow-400 transition">
                  {post.metadata.title}
                </h3>
                <p className="text-sm sm:text-base text-slate-400 leading-relaxed">
                  {post.metadata.description}
                </p>
              </div>
            </a>
          ))}
        </section>

      </main>

      {/* Footer */}
      <footer className="border-t border-slate-900 bg-slate-950/80 py-10 mt-32 text-center text-xs text-slate-600 relative z-10">
        <p>Kilovolt Telemetry Hub & Landing Server &copy; 2026. Powered by Next.js App Router.</p>
      </footer>
    </div>
  );
}
