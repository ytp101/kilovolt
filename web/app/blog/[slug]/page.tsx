import React from 'react';
import { promises as fs } from 'fs';
import path from 'path';
import { parseFrontmatter, compileMarkdown } from '../utils';
import { notFound } from 'next/navigation';

interface PageProps {
  params: Promise<{ slug: string }>;
}

// Generate static metadata header tags for search engines
export async function generateMetadata({ params }: PageProps) {
  const { slug } = await params;
  const contentDir = path.join(process.cwd(), 'content', 'blog');
  const filePath = path.join(contentDir, `${slug}.md`);

  try {
    const fileContent = await fs.readFile(filePath, 'utf8');
    const { metadata } = parseFrontmatter(fileContent);

    return {
      title: `${metadata.title} — Kilovolt`,
      description: metadata.description,
      openGraph: {
        title: metadata.title,
        description: metadata.description,
        type: 'article',
        publishedTime: metadata.date,
        url: `https://kilovolt.vercel.app/blog/${slug}`,
      },
    };
  } catch (e) {
    return {
      title: 'Post Not Found — Kilovolt',
      description: 'The requested engineering post could not be resolved.',
    };
  }
}

export default async function BlogPost({ params }: PageProps) {
  const { slug } = await params;
  const contentDir = path.join(process.cwd(), 'content', 'blog');
  const filePath = path.join(contentDir, `${slug}.md`);

  let fileContent: string;
  try {
    fileContent = await fs.readFile(filePath, 'utf8');
  } catch (e) {
    notFound();
  }

  const { metadata, content } = parseFrontmatter(fileContent);
  const htmlContent = compileMarkdown(content);

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
              href="/blog" 
              className="text-sm font-medium text-slate-400 hover:text-yellow-400 transition"
            >
              &larr; Back to Blog
            </a>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-3xl mx-auto px-4 sm:px-6 lg:px-8 py-16 relative z-10 space-y-8">
        
        {/* Post Metadata Card */}
        <div className="space-y-4 text-center sm:text-left border-b border-slate-900 pb-8">
          <div className="text-xs text-slate-500 font-mono">
            Published: {metadata.date}
          </div>
          <h1 className="text-3xl sm:text-5xl font-black tracking-tight text-white leading-tight">
            {metadata.title}
          </h1>
          <p className="text-slate-400 text-lg sm:text-xl font-light italic leading-relaxed">
            {metadata.description}
          </p>
        </div>

        {/* Compiled Markdown Body Container */}
        <article 
          className="prose prose-invert prose-yellow max-w-none space-y-6"
          dangerouslySetInnerHTML={{ __html: htmlContent }}
        />

        {/* Call to Action Container */}
        <div className="mt-16 bg-slate-900/30 border border-slate-900 rounded-2xl p-6 sm:p-8 space-y-6">
          <h3 className="text-lg font-bold text-slate-200">🛡️ Protect Your Production Systems</h3>
          <p className="text-sm text-slate-400 leading-relaxed">
            Stop worrying about unexpected API spend loops or memory bottlenecks. Install Kilovolt today in under 30 seconds.
          </p>
          <div className="space-y-2">
            <p className="text-xs text-slate-500 font-mono"># Pull and run the gateway instantly</p>
            <pre className="bg-slate-950/80 border border-slate-900 rounded-xl p-4 font-mono text-xs sm:text-sm text-yellow-400 overflow-x-auto">
              <code>{`docker run -d --name kilovolt-proxy -p 8080:8080 yodsarun/kilovolt-proxy:latest`}</code>
            </pre>
          </div>
          <div className="text-center">
            <a 
              href="https://github.com/ytp101/kilovolt" 
              target="_blank" 
              rel="noreferrer" 
              className="text-xs text-slate-500 hover:text-yellow-400 underline transition"
            >
              Star the repository on GitHub &rarr;
            </a>
          </div>
        </div>

      </main>

      {/* Footer */}
      <footer className="border-t border-slate-900 bg-slate-950/80 py-10 mt-32 text-center text-xs text-slate-600 relative z-10">
        <p>Kilovolt Telemetry Hub & Landing Server &copy; 2026. Powered by Next.js App Router.</p>
      </footer>
    </div>
  );
}
