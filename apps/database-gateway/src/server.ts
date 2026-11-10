import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { randomBytes, randomUUID } from "node:crypto";
import { WebSocket, WebSocketServer, type RawData } from "ws";
import { runtimeOrigin, type RuntimeAccess, type GatewayAccess } from "@spitzli/database-contracts";
import { managedIdentity, type ManagedRuntimeManager, type ManagedRuntime } from "./managed-runtimes";
import { authorizeRpc, publicResult, GatewayError, record } from "./policy";

export interface GatewayOptions {
  publicOrigin: string;
  allowedOrigins: string[];
  runtimeOrigins: string[];
  expectedCommit?: string;
  managedRuntimes?: ManagedRuntimeManager;
  control: (body: Record<string, string>) => Promise<unknown>;
  audit?: (event: { scopeID: string; subject: string; command: string; status: number; durationMs: number }) => void;
}
type Session = { id: string; inspecting?: Promise<RuntimeAccess>; ended?: Promise<void>; managed?: string; sensitive?: string[]; runtime?: ManagedRuntime; expiry?: NodeJS.Timeout; revalidation?: NodeJS.Timeout; cleanup: Set<() => void>; token: string; scopeID: string; expires: number; cookie?: string; csrf?: string; starting?: Promise<Record<string, unknown>>; negotiation?: Record<string, unknown>; active: number; queries: number };
const COOKIE = "webdock_database";
const MAX_BODY = 1_048_576, MAX_RESPONSE = 8_388_608;

async function readRequest(request: IncomingMessage): Promise<unknown> {
  if (request.headers["content-type"]?.split(";")[0] !== "application/json") throw new GatewayError(415, "Use application/json.");
  const parts: Buffer[] = []; let bytes = 0;
  for await (const part of request) {
    bytes += part.length;
    if (bytes > MAX_BODY) throw new GatewayError(413, "Database request is too large.");
    parts.push(Buffer.from(part));
  }
  try { return JSON.parse(Buffer.concat(parts).toString()); } catch { throw new GatewayError(400, "Invalid JSON."); }
}
async function boundedJSON(response: Response) {
  const reader = response.body?.getReader(); if (!reader) throw new GatewayError(502, "Empty database response.");
  const parts: Uint8Array[] = []; let size = 0;
  try {
    for (;;) { const next = await reader.read(); if (next.done) break; size += next.value.byteLength;
      if (size > MAX_RESPONSE) throw new GatewayError(413, "Database result is too large. Use pagination."); parts.push(next.value); }
    return JSON.parse(Buffer.concat(parts).toString()) as unknown;
  } finally { await reader.cancel().catch(() => {}); }
}

export function createGateway(options: GatewayOptions) {
  const origin = runtimeOrigin.parse(options.publicOrigin);
  const allowedOrigins = new Set(options.allowedOrigins.map(value => runtimeOrigin.parse(value)));
  const runtimeOrigins = new Set(options.runtimeOrigins.map(value => runtimeOrigin.parse(value)));
  const sessions = new Map<string, Session>();
  let requests = 0;
  const sockets = new WebSocketServer({ noServer: true, maxPayload: MAX_BODY });
  const upstreamSockets = new Set<WebSocket>();
  let upgrades = 0;
  const retire = (session: Session): Promise<void> => {
    if (session.ended) return session.ended;
    session.expires = 0; session.sensitive = undefined;
    clearTimeout(session.expiry); clearInterval(session.revalidation);
    for (const [key, value] of sessions) if (value === session) sessions.delete(key);
    for (const close of session.cleanup) close(); session.cleanup.clear();
    session.ended = options.managedRuntimes?.release(session.id) ?? Promise.resolve();
    return session.ended;
  };
  const inspectAccess = async (session: Session): Promise<RuntimeAccess> => {
    try {
      if (session.expires <= Date.now()) throw new GatewayError(401, "Database session expired.");
      let access: GatewayAccess;
      try { access = await options.control({ action: "inspect", token: session.token }) as GatewayAccess; }
      catch { throw new GatewayError(401, "Database access expired or was revoked."); }
      const expires = new Date(access.expiresAt).getTime();
      if (session.expires <= Date.now() || access.scopeID !== session.scopeID || !Number.isFinite(expires) || expires <= Date.now()) {
        throw new GatewayError(401, "Database runtime is not authorized.");
      }
      if ("kind" in access && access.kind === "managed") {
        if (!options.managedRuntimes) throw new GatewayError(503, "Managed database runtime unavailable.");
        const identity = managedIdentity(access);
        if (session.managed && session.managed !== identity) throw new GatewayError(401, "Database access changed. Open a new session.");
        session.managed = identity;
        session.sensitive = [access.database.password, access.database.host, access.database.username, access.database.caCertificate].filter(value => value.length > 0);
        const resolved = await options.managedRuntimes.acquire(session.id, access);
        if (session.expires <= Date.now()) throw new GatewayError(401, "Database session expired.");
        session.runtime = resolved;
        return resolved;
      }
      if (session.managed || !("runtimeOrigin" in access) || !runtimeOrigins.has(access.runtimeOrigin)) throw new GatewayError(403, "Database runtime is not authorized.");
      return access;
    } catch (error) { await retire(session); throw error; }
  };
  const inspect = (session: Session): Promise<RuntimeAccess> => {
    // Share concurrent checks only; the next request must observe revocation afresh.
    session.inspecting ??= inspectAccess(session).finally(() => { session.inspecting = undefined; });
    return session.inspecting;
  };
  const upstreamHeaders = (session: Session, access: RuntimeAccess) => ({
    "Content-Type": "application/json", Origin: session.runtime?.authenticationOrigin ?? access.runtimeOrigin,
    "X-Tabularis-User": access.subject, "X-Tabularis-Proxy-Secret": access.proxySecret,
    ...(session.cookie ? { Cookie: session.cookie } : {}),
    ...(session.csrf ? { "X-Tabularis-Csrf": session.csrf } : {}),
    "X-Tabularis-Deadline-Ms": "30000",
  });
  const negotiate = (session: Session, access: RuntimeAccess): Promise<Record<string, unknown>> => {
    if (session.negotiation) return Promise.resolve(session.negotiation);
    session.starting ??= (async () => {
      const response = await fetch(access.runtimeOrigin+"/api/v1/session", { headers: upstreamHeaders(session, access), redirect: "error", signal: AbortSignal.timeout(10_000) });
      if (!response.ok) throw new GatewayError(502, "Database runtime authentication failed.");
      const data = record(await boundedJSON(response));
      if (data.apiVersion !== "v1" || data.authenticated !== true || typeof data.csrfToken !== "string" || !data.csrfToken ||
        (options.expectedCommit && record(data.serverBuild).commit !== options.expectedCommit)) throw new GatewayError(502, "Incompatible database runtime.");
      const cookie = response.headers.getSetCookie().find(value => value.startsWith("tabularis_session="))?.split(";")[0];
      if (!cookie) throw new GatewayError(502, "Database runtime did not issue a session.");
      session.cookie = cookie; session.csrf = data.csrfToken;
      session.negotiation = { ...data, access: { remote: true, authorizationLevel: "database", highRiskCapabilities: false },
        capabilities: { ...record(data.capabilities), rpc: true, events: true, uploads: false, downloads: false, pluginAssets: false, mcpHostConfiguration: false, serverFileBrowser: false, nativeUpdater: false },
        queryResponsePolicy: { ...record(data.queryResponsePolicy), maxResponseBytes: MAX_RESPONSE },
      };
      return session.negotiation;
    })().catch(async error => { session.starting = undefined; if (session.managed) await retire(session); throw error; });
    return session.starting;
  };
  const locate = (req: IncomingMessage) => {
    const match = req.url?.match(/^\/s\/([1-9][0-9]{0,18})(\/api\/v1\/[a-z_/-]+)$/);
    const cookie = req.headers.cookie?.split(";").map(value => value.trim()).find(value => value.startsWith(COOKIE+"="))?.slice(COOKIE.length+1);
    const session = cookie && sessions.get(cookie);
    if (!match || !session || session.scopeID !== match[1]) throw new GatewayError(401, "Database session required.");
    return { session, path: match[2], cookie };
  };
  const checkOrigin = (req: IncomingMessage) => {
    if (!req.headers.origin || !allowedOrigins.has(req.headers.origin)) throw new GatewayError(403, "Invalid origin.");
  };
  const json = (res: ServerResponse, status: number, value: unknown) => {
    res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" });
    res.end(JSON.stringify(value));
  };
  const server = createServer(async (req, res) => {
    let active: Session | undefined;
    let query = false;
    try {
      if (req.url === "/healthz" && req.method === "GET") return json(res, 200, { ok: true });
      checkOrigin(req);
      res.setHeader("Access-Control-Allow-Origin", req.headers.origin!);
      res.setHeader("Access-Control-Allow-Credentials", "true"); res.setHeader("Vary", "Origin");
      if (req.method === "OPTIONS") {
        res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
        res.setHeader("Access-Control-Allow-Headers", "content-type,x-request-id,x-tabularis-csrf,x-tabularis-deadline-ms,x-tabularis-cancellation-id");
        res.writeHead(204); return res.end();
      }
      if (req.url === "/session" && req.method === "POST") {
        for (const session of sessions.values()) if (session.expires <= Date.now()) await retire(session);
        if (sessions.size >= 256) throw new GatewayError(503, "Database gateway is at capacity.");
        const body = record(await readRequest(req));
        if (typeof body.code !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(body.code) || Object.keys(body).length !== 1) throw new GatewayError(400, "Invalid launch code.");
        let exchange: Record<string, unknown>;
        try { exchange = record(await options.control({ action: "exchange", code: body.code })); } catch { throw new GatewayError(401, "Launch code expired or was already used."); }
        if (typeof exchange.token !== "string" || typeof exchange.scopeID !== "string" || !/^[1-9][0-9]{0,18}$/.test(exchange.scopeID)) throw new GatewayError(502, "Invalid session response.");
        const session: Session = { id: randomUUID(), cleanup: new Set(), token: exchange.token, scopeID: exchange.scopeID, expires: new Date(String(exchange.expiresAt)).getTime(), active: 0, queries: 0 };
        if (!Number.isFinite(session.expires) || session.expires <= Date.now()) throw new GatewayError(401, "Session expired.");
        await inspect(session);
        const cookie = randomBytes(32).toString("base64url"); sessions.set(cookie, session);
        session.expiry = setTimeout(() => { void retire(session); }, Math.min(session.expires - Date.now(), 2_147_483_647));
        session.expiry.unref();
        if (session.managed) {
          let checking = false;
          session.revalidation = setInterval(async () => { if (checking) return; checking = true; try { await inspect(session); } catch {} finally { checking = false; } }, 3000);
          session.revalidation.unref();
        }
        res.setHeader("Set-Cookie", `${COOKIE}=${cookie}; Path=/s/${session.scopeID}/; HttpOnly; SameSite=Strict; Max-Age=${Math.min(900, Math.floor((session.expires-Date.now())/1000))}${origin.startsWith("https:") ? "; Secure" : ""}`);
        return json(res, 200, { scopeID: session.scopeID, connectionID: session.scopeID, baseUrl: `${origin}/s/${session.scopeID}`, expiresAt: new Date(session.expires).toISOString() });
      }
      const { session, path, cookie } = locate(req);
      if (session.active >= 16 || requests >= 64) throw new GatewayError(429, "Too many concurrent database operations.");
      session.active++; requests++; active = session;
      const access = await inspect(session);
      if (path === "/api/v1/session" && req.method === "GET") return json(res, 200, await negotiate(session, access));
      if (req.method !== "POST") throw new GatewayError(405, "Method not allowed.");
      await negotiate(session, access);
      if (req.headers["x-tabularis-csrf"] !== session.csrf) throw new GatewayError(403, "Invalid database CSRF token.");
      if (path === "/api/v1/logout") {
        sessions.delete(cookie);
        await fetch(access.runtimeOrigin+"/api/v1/logout", { method: "POST", headers: upstreamHeaders(session, access), redirect: "error", signal: AbortSignal.timeout(2000) }).catch(() => {});
        await retire(session);
        await options.control({ action: "logout", token: session.token });
        return json(res, 200, { ok: true });
      }
      const command = path.match(/^\/api\/v1\/rpc\/([a-z_]+)$/)?.[1];
      if (!command) throw new GatewayError(403, "This database endpoint is not permitted.");
      const started = Date.now();
      res.once("finish", () => options.audit?.({ scopeID: access.scopeID, subject: access.subject, command: command.slice(0, 120), status: res.statusCode, durationMs: Date.now()-started }));
      let input = authorizeRpc(command, await readRequest(req), access);
      if (["test_connection", "execute_query", "execute_query_batch", "count_query", "explain_query_plan"].includes(command)) {
        if (session.queries >= 4) throw new GatewayError(429, "Too many concurrent queries.");
        session.queries++; query = true;
      }
      // Remote test_connection rejects inline host certificate paths. Probe the saved
      // connection instead so the runtime retains its verified TLS and credentials.
      const upstreamPath = command === "test_connection" ? "/api/v1/rpc/execute_query" : path;
      if (command === "test_connection") input = { connectionId: access.connectionID, query: "SELECT 1", limit: 1 };
      const headers: Record<string, string> = upstreamHeaders(session, access);
      headers["x-request-id"] = randomUUID();
      for (const name of ["x-request-id", "x-tabularis-cancellation-id"]) {
        const value = req.headers[name]; if (typeof value === "string" && /^[a-zA-Z0-9_-]{1,128}$/.test(value)) headers[name] = value;
      }
      const abort = new AbortController();
      let revoked = false, checking = false;
      const cancel = () => {
        abort.abort();
        if (query) void fetch(access.runtimeOrigin+"/api/v1/rpc/cancel_query", {
          method: "POST", headers: upstreamHeaders(session, access), redirect: "error", signal: AbortSignal.timeout(2000),
          body: JSON.stringify({ connectionId: access.connectionID, queryRequestId: headers["x-request-id"] }),
        }).catch(() => {});
      };
      session.cleanup.add(cancel);
      const timeout = setTimeout(cancel, 35_000);
      const revalidate = setInterval(async () => {
        if (checking || revoked) return; checking = true;
        try { await inspect(session); } catch { revoked = true; cancel(); } finally { checking = false; }
      }, 3000);
      const disconnected = () => { if (!res.writableEnded) cancel(); };
      res.on("close", disconnected);
      try {
        const response = await fetch(access.runtimeOrigin+upstreamPath, { method: "POST", headers, body: JSON.stringify(input), redirect: "error", signal: abort.signal });
        const result = record(await boundedJSON(response));
        await inspect(session);
        if (session.managed && result.ok === false) {
          result.error = result.error ? JSON.parse(JSON.stringify(result.error), (_key, value) => {
            if (typeof value !== "string") return value;
            for (const secret of session.sensitive ?? []) value = value.replaceAll(secret, "[redacted]");
            return value;
          }) : { code: "DATABASE_RUNTIME_ERROR", message: "Database operation failed." };
        }
        if (result.ok === true) result.data = command === "test_connection" ? "Connection successful!" : publicResult(command, result.data, access);
        return json(res, response.status, result);
      } catch (error) {
        if (revoked) throw new GatewayError(401, "Database access expired or was revoked.");
        throw error;
      } finally { session.cleanup.delete(cancel); clearTimeout(timeout); clearInterval(revalidate); res.off("close", disconnected); }
    } catch (error) {
      if (!res.destroyed && !res.headersSent) json(res, error instanceof GatewayError ? error.status : 502,
        { ok: false, error: { code: "DATABASE_GATEWAY_ERROR", message: error instanceof GatewayError ? error.message : "Database request failed. Check its outcome before retrying.", details: null, requestId: randomBytes(8).toString("hex") } });
    } finally { if (active) { active.active--; requests--; if (query) active.queries--; } }
  });
  server.on("upgrade", async (req, socket, head) => {
    if (upgrades >= 16 || sockets.clients.size >= 64) { socket.destroy(); return; }
    upgrades++;
    try {
      checkOrigin(req); const { session, path } = locate(req);
      if (path !== "/api/v1/events") throw new GatewayError(403, "Events only.");
      const access = await inspect(session); await negotiate(session, access);
      sockets.handleUpgrade(req, socket, head, client => {
        const target = new URL("/api/v1/events", access.runtimeOrigin); target.protocol = target.protocol === "https:" ? "wss:" : "ws:";
        const upstream = new WebSocket(target, { headers: upstreamHeaders(session, access), maxPayload: MAX_RESPONSE });
        upstreamSockets.add(upstream);
        const pending: string[] = []; let pendingBytes = 0, closed = false, checking = false;
        const close = () => { if (closed) return; closed = true; clearInterval(timer); upstreamSockets.delete(upstream); session.cleanup.delete(close); client.close(1008, "Database session ended"); upstream.close();
          const terminate = setTimeout(() => { client.terminate(); upstream.terminate(); }, 1000); terminate.unref(); };
        session.cleanup.add(close);
        const timer = setInterval(async () => { if (checking) return; checking = true; try { await inspect(session); } catch { close(); } finally { checking = false; } }, 3000);
        upstream.on("open", () => { for (const message of pending) upstream.send(message); pending.length = 0; });
        // Authorization is asynchronous; preserve wire order in each direction.
        const ordered = (maximum: number, forward: (text: string) => void) => {
          let queue = Promise.resolve(), queuedBytes = 0, queuedFrames = 0;
          return (data: RawData) => {
            if (closed) return;
            const text = data.toString(), bytes = Buffer.byteLength(text);
            if (queuedBytes + bytes > maximum || queuedFrames >= 128) { close(); return; }
            queuedBytes += bytes; queuedFrames++;
            queue = queue.then(async () => {
              if (closed) return;
              await inspect(session);
              if (!closed) forward(text);
            }).catch(close).finally(() => { queuedBytes -= bytes; queuedFrames--; });
          };
        };
        client.on("message", ordered(MAX_BODY, text => {
          const message = record(JSON.parse(text));
          if (!["subscribe", "unsubscribe", "ping"].includes(String(message.type))) throw Error();
          const encoded = JSON.stringify(message);
          if (upstream.readyState === WebSocket.OPEN) {
            if (upstream.bufferedAmount + Buffer.byteLength(encoded) > MAX_BODY) throw Error();
            upstream.send(encoded);
          } else { pendingBytes += Buffer.byteLength(encoded); if (pendingBytes > MAX_BODY) throw Error(); pending.push(encoded); }
        }));
        upstream.on("message", ordered(MAX_RESPONSE, text => {
          const event = record(JSON.parse(text));
          if (event.payload && typeof event.payload === "object" && !Array.isArray(event.payload)) {
            const payload = { ...record(event.payload) };
            for (const field of ["connectionId", "connection_id"]) if (payload[field] === access.connectionID) payload[field] = access.scopeID;
            event.payload = payload;
          }
          if (client.readyState === WebSocket.OPEN) {
            const encoded = JSON.stringify(event);
            if (client.bufferedAmount + Buffer.byteLength(encoded) > MAX_RESPONSE) throw Error();
            client.send(encoded);
          }
        }));
        client.on("close", close); client.on("error", close); upstream.on("close", close); upstream.on("error", close);
      });
    } catch { socket.write("HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n"); socket.destroy(); }
    finally { upgrades--; }
  });
  server.on("close", () => { for (const client of sockets.clients) client.terminate(); sockets.close(); void Promise.all([...sessions.values()].map(retire)).then(() => options.managedRuntimes?.shutdown()); });
  server.requestTimeout = 15_000;
  server.headersTimeout = 15_000;
  return Object.assign(server, { shutdown: async () => {
    await Promise.all([...sessions.values()].map(retire));
    await options.managedRuntimes?.shutdown();
    await new Promise<void>(resolve => {
    for (const client of sockets.clients) client.terminate();
    for (const upstream of upstreamSockets) upstream.terminate();
    server.close(() => resolve()); server.closeAllConnections();
    });
  } });
}
