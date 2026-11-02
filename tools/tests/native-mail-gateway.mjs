// Fixture-only fixed HTTP management hop. Importing this module starts no listener.
import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function validateGatewayConfig(value) {
  const deny = () => { throw Error('invalid_fixture_gateway_configuration'); };
  if (!value || Object.keys(value).some(k => !['version', 'runID', 'adminDomain', 'upstream', 'maxBodyBytes', 'timeoutMs'].includes(k))) deny();
  if (value.version !== 1 || !/^[a-f0-9]{12}$/.test(value.runID) || value.adminDomain !== `mail-admin-${value.runID}.localhost`) deny();
  const url = new URL(value.upstream), ip = url.hostname.split('.').map(Number);
  const privateIP = ip.length === 4 && ip.every(n => Number.isInteger(n) && n >= 0 && n < 256) && (ip[0] === 10 || ip[0] === 172 && ip[1] >= 16 && ip[1] < 32 || ip[0] === 192 && ip[1] === 168);
  if (!privateIP || url.protocol !== 'http:' || url.port && url.port !== '80' || url.username || url.password || url.pathname !== '/' || url.search || url.hash) deny();
  if (!Number.isInteger(value.maxBodyBytes) || value.maxBodyBytes < 1 || value.maxBodyBytes > 1048576 || !Number.isInteger(value.timeoutMs) || value.timeoutMs < 1 || value.timeoutMs > 10000) deny();
  return { ...value, upstream: url.origin };
}
function error(status, code, headers) { return new Response(code, { status, headers }); }
async function boundedBody(body, max, signal) {
  if (!body) return null;
  const reader = body.getReader(), chunks = []; let length = 0;
  const aborted = new Promise((_, reject) => signal.addEventListener('abort', () => reject(Error('deadline')), { once: true }));
  try {
    while (true) {
      const part = await Promise.race([reader.read(), aborted]); if (part.done) break;
      length += part.value.byteLength; if (length > max) throw Error('body_limit'); chunks.push(part.value);
    }
    const bytes = new Uint8Array(length); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; } return bytes;
  } catch (failure) { void reader.cancel().catch(() => {}); throw failure; }
}
export function gatewayHandler(configuration, fetchUpstream = fetch) {
  const config = validateGatewayConfig(configuration);
  const suffix = '.' + config.adminDomain;
  return async request => {
    const url = new URL(request.url);
    if (url.search || !['GET', 'POST'].includes(request.method)) return error(400, 'request_denied');
    if (request.method === 'GET' && url.pathname === '/healthz') return new Response('ok');
    const host = (request.headers.get('host') ?? '').toLowerCase(), prefix = host.endsWith(suffix) ? host.slice(0, -suffix.length) : '';
    if (!/^c[1-9][0-9]{0,18}$/.test(prefix)) return error(403, 'host_denied');
    if (!(request.method === 'POST' && url.pathname === '/jmap' || request.method === 'GET' && ['/jmap/session', '/.well-known/jmap'].includes(url.pathname))) return error(404, 'path_denied');
    const rawLength = request.headers.get('content-length');
    if (rawLength !== null && (!/^[0-9]+$/.test(rawLength) || Number(rawLength) > config.maxBodyBytes)) return error(413, 'body_limit');
    if (request.method === 'GET' && rawLength !== null && Number(rawLength) !== 0) return error(400, 'request_denied');
    const controller = new AbortController();
    const deadline = new Promise(resolveDeadline => controller.signal.addEventListener('abort', () => resolveDeadline(error(504, 'upstream_deadline')), { once: true }));
    const timer = setTimeout(() => controller.abort(), config.timeoutMs);
    const forward = async () => {
      let body;
      try { body = await boundedBody(request.body, config.maxBodyBytes, controller.signal); }
      catch { return error(controller.signal.aborted ? 504 : 413, controller.signal.aborted ? 'upstream_deadline' : 'body_limit'); }
      const headers = new Headers({ host });
      for (const name of ['authorization', 'content-type', 'accept']) { const value = request.headers.get(name); if (value !== null) headers.set(name, value); }
      try {
        const response = await fetchUpstream(config.upstream + url.pathname, { method: request.method, headers, ...(body === null ? {} : { body }), redirect: 'manual', signal: controller.signal });
        const replyHeaders = new Headers({ 'x-webdock-fixture-upstream-status': String(response.status) });
        if (response.status >= 300 && response.status < 400) { void response.body?.cancel().catch(() => {}); return error(502, 'upstream_redirect_denied', replyHeaders); }
        const bytes = await boundedBody(response.body, config.maxBodyBytes, controller.signal);
        if (response.headers.has('content-type')) replyHeaders.set('content-type', response.headers.get('content-type'));
        return new Response(bytes, { status: response.status, headers: replyHeaders });
      } catch { return error(controller.signal.aborted ? 504 : 502, controller.signal.aborted ? 'upstream_deadline' : 'upstream_unavailable'); }
    };
    try { return await Promise.race([forward(), deadline]); } finally { clearTimeout(timer); }
  };
}
async function main(path) {
  if (!isAbsolute(path ?? '')) throw Error('private_gateway_config_required');
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW); let config;
  try { const s = await file.stat(); if (!s.isFile() || s.nlink !== 1 || s.uid !== process.getuid() || s.mode & 0o077 || s.size > 16384) throw Error('private_gateway_config_required'); config = JSON.parse(await file.readFile('utf8')); }
  finally { await file.close(); }
  const handle = gatewayHandler(config);
  const server = globalThis.Bun.serve({ hostname: '0.0.0.0', port: 8080, maxRequestBodySize: config.maxBodyBytes, idleTimeout: 10, fetch: handle, error: () => error(500, 'gateway_unavailable') });
  const stop = () => { server.stop(true); process.exit(0); };
  process.on('SIGTERM', stop); process.on('SIGINT', stop);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv[2]).catch(() => { process.stderr.write('fixture_mail_gateway_failed\n'); process.exitCode = 1; });
