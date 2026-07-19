import { promises as fs } from 'fs';
import path from 'path';

export interface PostMetadata {
  title: string;
  description: string;
  slug: string;
  date: string;
}

export interface Post {
  metadata: PostMetadata;
  content: string;
}

// Simple YAML frontmatter parser
export function parseFrontmatter(rawContent: string): Post {
  const frontmatterRegex = /^---\r?\n([\s\S]+?)\r?\n---\r?\n([\s\S]*)$/;
  const match = rawContent.match(frontmatterRegex);

  if (!match) {
    return {
      metadata: {
        title: 'Untitled Post',
        description: '',
        slug: '',
        date: new Date().toISOString().split('T')[0],
      },
      content: rawContent,
    };
  }

  const [, yamlBlock, content] = match;
  const metadata: any = {};

  yamlBlock.split('\n').forEach((line) => {
    const parts = line.split(':');
    if (parts.length >= 2) {
      const key = parts[0].trim();
      const val = parts.slice(1).join(':').trim().replace(/^['"]|['"]$/g, '');
      metadata[key] = val;
    }
  });

  return {
    metadata: {
      title: metadata.title || 'Untitled Post',
      description: metadata.description || '',
      slug: metadata.slug || '',
      date: metadata.date || new Date().toISOString().split('T')[0],
    },
    content,
  };
}

// Simple Markdown to HTML compiler
export function compileMarkdown(markdown: string): string {
  let html = '';
  const lines = markdown.split(/\r?\n/);
  let inList = false;
  let listType: 'ul' | 'ol' | null = null;
  let inCodeBlock = false;
  let codeBlockContent: string[] = [];
  let codeBlockLang = '';

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // Handle code blocks
    if (line.trim().startsWith('```')) {
      if (inCodeBlock) {
        // End of code block
        inCodeBlock = false;
        const codeEscaped = codeBlockContent
          .join('\n')
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');
        html += `<pre class="bg-slate-950/60 border border-slate-900 rounded-xl p-4 font-mono text-xs sm:text-sm text-slate-300 overflow-x-auto my-6"><code class="language-${codeBlockLang}">${codeEscaped}</code></pre>`;
        codeBlockContent = [];
        codeBlockLang = '';
      } else {
        // Start of code block
        inCodeBlock = true;
        codeBlockLang = line.trim().slice(3).trim();
      }
      continue;
    }

    if (inCodeBlock) {
      codeBlockContent.push(line);
      continue;
    }

    // Close open list if line is not a list item
    const isUnordered = line.trim().startsWith('- ') || line.trim().startsWith('* ');
    const isOrdered = /^\d+\.\s/.test(line.trim());

    if (inList && !isUnordered && !isOrdered) {
      html += listType === 'ul' ? '</ul>' : '</ol>';
      inList = false;
      listType = null;
    }

    // Handle empty lines (paragraph breakers)
    if (line.trim() === '') {
      continue;
    }

    // Handle Headings
    if (line.startsWith('# ')) {
      html += `<h1 class="text-3xl sm:text-4xl font-extrabold text-white mt-10 mb-4 tracking-tight">${processInlineStyles(line.slice(2))}</h1>`;
    } else if (line.startsWith('## ')) {
      html += `<h2 class="text-xl sm:text-2xl font-bold text-slate-200 mt-8 mb-4 border-b border-slate-900 pb-2">${processInlineStyles(line.slice(3))}</h2>`;
    } else if (line.startsWith('### ')) {
      html += `<h3 class="text-lg font-bold text-slate-300 mt-6 mb-3">${processInlineStyles(line.slice(4))}</h3>`;
    } 
    // Handle Blockquotes
    else if (line.startsWith('> ')) {
      html += `<blockquote class="border-l-4 border-yellow-500 bg-slate-900/30 px-4 py-3 my-4 italic text-slate-400 rounded-r-lg">${processInlineStyles(line.slice(2))}</blockquote>`;
    }
    // Handle list items
    else if (isUnordered) {
      if (!inList) {
        html += '<ul class="list-disc pl-6 space-y-2 my-4 text-slate-300 text-sm sm:text-base">';
        inList = true;
        listType = 'ul';
      }
      const itemContent = line.trim().slice(2);
      html += `<li>${processInlineStyles(itemContent)}</li>`;
    } else if (isOrdered) {
      if (!inList) {
        html += '<ol class="list-decimal pl-6 space-y-2 my-4 text-slate-300 text-sm sm:text-base">';
        inList = true;
        listType = 'ol';
      }
      const itemContent = line.trim().replace(/^\d+\.\s/, '');
      html += `<li>${processInlineStyles(itemContent)}</li>`;
    }
    // Handle standard Paragraphs
    else {
      html += `<p class="text-sm sm:text-base text-slate-400 leading-relaxed my-4">${processInlineStyles(line)}</p>`;
    }
  }

  // Close remaining list if any
  if (inList) {
    html += listType === 'ul' ? '</ul>' : '</ol>';
  }

  return html;
}

// Inline formatting (bold, links, code tags)
function processInlineStyles(text: string): string {
  let formatted = text;

  // Escape special tags safely to avoid breaks
  formatted = formatted
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');

  // Restore links escaping
  formatted = formatted
    .replace(/&lt;a /g, '<a ')
    .replace(/&lt;\/a&gt;/g, '</a>')
    .replace(/href="&amp;/g, 'href="&');

  // Bold (**text**)
  formatted = formatted.replace(/\*\*(.*?)\*\*/g, '<strong class="text-slate-100 font-bold">$1</strong>');

  // Inline code (`code`)
  formatted = formatted.replace(/`(.*?)`/g, '<code class="bg-slate-900 text-yellow-400 px-1.5 py-0.5 rounded font-mono text-xs border border-slate-800">$1</code>');

  // Links ([text](url))
  formatted = formatted.replace(/\[(.*?)\]\((.*?)\)/g, '<a href="$2" class="text-yellow-400 hover:underline transition font-semibold" target="_self">$1</a>');

  return formatted;
}

// Statically fetch all blog posts from content directory
export async function getAllPosts(): Promise<Post[]> {
  const contentDir = path.join(process.cwd(), 'content', 'blog');
  
  try {
    const filenames = await fs.readdir(contentDir);
    const mdFiles = filenames.filter((file) => file.endsWith('.md'));
    
    const posts = await Promise.all(
      mdFiles.map(async (filename) => {
        const filePath = path.join(contentDir, filename);
        const fileContent = await fs.readFile(filePath, 'utf8');
        return parseFrontmatter(fileContent);
      })
    );
    
    // Sort chronological descending
    return posts.sort((a, b) => b.metadata.date.localeCompare(a.metadata.date));
  } catch (e) {
    return [];
  }
}
