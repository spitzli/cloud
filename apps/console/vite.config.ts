import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwind from '@tailwindcss/vite';
// Local HTTPS (WEBDOCK_DEV_HTTPS) uses certificates from a locally trusted CA, e.g. mkcert.
const cert = process.env.WEBDOCK_DEV_TLS_CERT;
const key = process.env.WEBDOCK_DEV_TLS_KEY;
const core = process.env.WEBDOCK_DEV_API_ORIGIN ?? 'http://127.0.0.1:3130';
const coreURL = new URL(core);
if (coreURL.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(coreURL.hostname) || coreURL.username || coreURL.password || coreURL.pathname !== '/' || coreURL.search || coreURL.hash) throw new Error('Local Core origin required for the development proxy');
export default defineConfig({
  // Parallel owned fixtures use another Core origin and must not invalidate Tilt's optimized modules.
  ...(core !== 'http://127.0.0.1:3130' ? { cacheDir: fileURLToPath(new URL(`../../.runtime-test/vite-cache/${createHash('sha256').update(core).digest('hex').slice(0, 12)}`, import.meta.url)) } : {}),
  plugins: [react(), tailwind()],
  // First-party plugin UIs live in plugins/<id>/ui and share the Console's React and SDK.
  resolve: {
    alias: {
      '@webdock/plugin-ui': fileURLToPath(new URL('./src/plugins/sdk.ts', import.meta.url)),
      '@plugins': fileURLToPath(new URL('../../plugins', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@tanstack/react-query', 'zod'],
  },
  build: { target: 'es2022' },
  server: {
    host: '127.0.0.1', port: 3120, strictPort: true,
    ...(cert && key ? { https: { cert: readFileSync(cert), key: readFileSync(key) } } : {}),
    proxy: {
      // Every Core path keeps its Host header; Core decides which surface may use it.
      '^/(api|v1|\\.well-known)/': { target: core, changeOrigin: false },
      '/console-config.json': { target: core, changeOrigin: false },
    },
  },
});
