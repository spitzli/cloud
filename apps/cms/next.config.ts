import path from 'node:path';
import type { NextConfig } from 'next';
const nextConfig: NextConfig = {
  transpilePackages:['@spitzli/i18n'],
  turbopack: { root: path.resolve(process.cwd(), '../..') },
  async headers() { return [{ source: '/:path*', headers: [{ key: 'X-Robots-Tag', value: 'noindex, nofollow' }] }]; },
};
export default nextConfig;
