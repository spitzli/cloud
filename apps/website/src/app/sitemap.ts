import type { MetadataRoute } from 'next';
export default function sitemap(): MetadataRoute.Sitemap { return ['', '/terms', '/privacy', '/imprint'].map(path => ({ url: `https://spitzli.cloud${path}`, changeFrequency: 'monthly', priority: path ? 0.3 : 1 })); }
