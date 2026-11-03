// Serves only the already-built fixture, on loopback, with a bounded lifetime.
import { createServer } from "node:http";
import { readFile, realpath, stat } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const directory = await realpath(process.env.NEON_PLUGIN_FIXTURE_DIR ?? fileURLToPath(new URL("../../../.runtime-test/neon-plugin-fixture", import.meta.url)));
await stat(resolve(directory, "tests/neon-plugin-fixture.html"));
const port = Number(process.env.NEON_PLUGIN_FIXTURE_PORT ?? 3138);
const lifetime = Number(process.env.NEON_PLUGIN_FIXTURE_LIFETIME_MS ?? 300_000);
if (!Number.isInteger(port) || port < 1024 || port > 65535 || !Number.isInteger(lifetime) || lifetime < 1000 || lifetime > 900_000) throw new Error("Expected a port from 1024–65535 and lifetime from 1000–900000ms");
const types = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".png": "image/png", ".woff2": "font/woff2" };
const server = createServer(async (request, response) => {
  try {
    if (!["GET", "HEAD"].includes(request.method ?? "")) { response.writeHead(405).end(); return; }
    const pathname = decodeURIComponent(new URL(request.url ?? "/", "http://127.0.0.1").pathname);
    const file = await realpath(resolve(directory, `.${pathname}`));
    if (!file.startsWith(`${directory}${sep}`) || !(await stat(file)).isFile()) { response.writeHead(404).end(); return; }
    response.writeHead(200, { "Content-Type": types[extname(file)] ?? "application/octet-stream", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" });
    response.end(request.method === "HEAD" ? undefined : await readFile(file));
  } catch {
    response.writeHead(404).end();
  }
});
const close = () => { server.closeAllConnections(); server.close(); };
process.once("SIGINT", close);
process.once("SIGTERM", close);
server.listen(port, "127.0.0.1", () => { console.log(`Synthetic Neon fixture: http://127.0.0.1:${port}/tests/neon-plugin-fixture.html (expires in ${lifetime}ms)`); });
setTimeout(close, lifetime).unref();
