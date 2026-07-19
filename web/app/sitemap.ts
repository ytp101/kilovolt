import { MetadataRoute } from 'next';
import { promises as fs } from 'fs';
import path from 'path';

export default async function sitemap(): Promise<MetadataRoute.Sitemap> {
  const baseUrl = 'https://kilovolt.vercel.app';

  // 1. Core static routes
  const staticRoutes: MetadataRoute.Sitemap = [
    {
      url: baseUrl,
      lastModified: new Date(),
      changeFrequency: 'weekly',
      priority: 1.0,
    },
    {
      url: `${baseUrl}/blog`,
      lastModified: new Date(),
      changeFrequency: 'daily',
      priority: 0.8,
    },
  ];

  // 2. Programmatically fetch and append dynamic blog routes
  const contentDir = path.join(process.cwd(), 'content', 'blog');
  let dynamicRoutes: MetadataRoute.Sitemap = [];

  try {
    const filenames = await fs.readdir(contentDir);
    const mdFiles = filenames.filter((file) => file.endsWith('.md'));

    dynamicRoutes = mdFiles.map((filename) => {
      const slug = filename.replace(/\.md$/, '');
      return {
        url: `${baseUrl}/blog/${slug}`,
        lastModified: new Date(),
        changeFrequency: 'monthly',
        priority: 0.6,
      };
    });
  } catch (e) {
    // Falls back safely if content directory is not found during initial build check
  }

  return [...staticRoutes, ...dynamicRoutes];
}
