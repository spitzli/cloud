/** Local verification only: real database services with a deterministic identity provider. */
import { createServer as httpServer, request as httpRequest } from "node:http";
import { createServer as httpsServer } from "node:https";
import { readFileSync } from "node:fs";
import { Pool, type PoolClient } from "pg";
import { sealData } from "iron-session";
import { WebSocket, WebSocketServer } from "ws";
import { executeDatabase, exchangeLaunch, inspectSession } from "../legacy/control-plane/src/modules/databases/service";
import { createGateway } from "../apps/database-gateway/src/server";

const url = new URL(process.env.WEBDOCK_DATABASE_TEST_URL || "");
if (process.env.NODE_ENV === "production" || url.hostname !== "127.0.0.1" || url.port !== "55441" || url.pathname !== "/webdock_admin_test") throw Error("Dedicated local fixture required");
const pool = new Pool({ connectionString: url.href });
const run = async <T>(fn: (db: PoolClient) => Promise<T>): Promise<T> => {
  const db = await pool.connect(); try { await db.query("BEGIN"); const result = await fn(db); await db.query("COMMIT"); return result; }
  catch (error) { await db.query("ROLLBACK"); throw error; } finally { db.release(); }
};
await pool.query(`ALTER TABLE webdock_auth."user" ADD COLUMN IF NOT EXISTS name text DEFAULT 'Database tester';
  ALTER TABLE webdock_auth."user" ADD COLUMN IF NOT EXISTS email text DEFAULT 'tester@example.invalid';
  UPDATE webdock_auth.session SET "expiresAt"=now()+interval '2 hours';
  DELETE FROM webdock_auth.studio_tenant_preview;
  CREATE SCHEMA IF NOT EXISTS browser_demo;
  CREATE TABLE IF NOT EXISTS browser_demo.orders(id integer PRIMARY KEY,customer text,total numeric(10,2),status text);
  INSERT INTO browser_demo.orders VALUES(1,'Anna Keller',49.90,'paid'),(2,'Ben Fischer',129.00,'pending'),(3,'Cem Yılmaz',75.50,'paid') ON CONFLICT DO NOTHING;
  DO $$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='browser_demo_reader') THEN CREATE ROLE browser_demo_reader LOGIN PASSWORD 'local_database_reader_only'; END IF; END $$;
  GRANT USAGE ON SCHEMA browser_demo TO browser_demo_reader;
  GRANT SELECT ON ALL TABLES IN SCHEMA browser_demo TO browser_demo_reader;
  DO $$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='browser_demo_writer') THEN CREATE ROLE browser_demo_writer LOGIN PASSWORD 'local_database_writer_only'; END IF; END $$;
  GRANT USAGE ON SCHEMA browser_demo TO browser_demo_writer;
  GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA browser_demo TO browser_demo_writer;
`);
const binding = (await pool.query("SELECT id FROM webdock_auth.database_binding ORDER BY id LIMIT 1")).rows[0];
if (!binding) throw Error("Run the database integration fixture first");
await run(db => executeDatabase(db, { subject: "operator", sessionID: "operator", source: "studio", scopes: ["hosting:read", "hosting:write"] }, {
  action: "grant", bindingID: binding.id, subject: "alice", profile: "read", runtimeOrigin: "https://localhost:3139", connectionID: "11111111-1111-4111-8111-111111111111",
  proxySecret: "local-tabularis-proxy-secret-1234567890", isolationVerified: true, databaseRoleVerified: true,
}));
await run(db => executeDatabase(db, { subject: "operator", sessionID: "operator", source: "studio", scopes: ["hosting:read", "hosting:write"] }, {
  action: "grant", bindingID: binding.id, subject: "bob", profile: "write", runtimeOrigin: "https://localhost:3159", connectionID: "22222222-2222-4222-8222-222222222222",
  proxySecret: "local-tabularis-writer-secret-1234567890", isolationVerified: true, databaseRoleVerified: true,
}));
const issuer = "http://localhost:3136/api/auth";
const auth = httpServer(async (req, res) => {
  const json = (status: number, data: unknown) => { res.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" }); res.end(JSON.stringify(data)); };
  try {
    if (req.url?.startsWith("/fixture/")) {
      const subject = req.url.endsWith("/bob") ? "bob" : req.url.endsWith("/eve") ? "eve" : "alice";
      const cookie = await sealData({ kind: "session", accessToken: subject+"-fixture-token", sub: subject, exp: Math.floor(Date.now()/1000)+3600 }, { password: process.env.WEBDOCK_SSO_COOKIE_SECRET!, ttl: 3600 });
      res.writeHead(302, { "Set-Cookie": `webdock-sso=${cookie}; Path=/; HttpOnly; SameSite=Lax`, Location: "http://localhost:3130/tenants/100/databases" }); return res.end();
    }
    if (req.url?.includes(".well-known/openid-configuration")) return json(200, { issuer, authorization_endpoint: issuer+"/authorize", token_endpoint: issuer+"/token", introspection_endpoint: issuer+"/oauth2/introspect", jwks_uri: issuer+"/jwks", response_types_supported: ["code"], subject_types_supported: ["public"], id_token_signing_alg_values_supported: ["RS256"] });
    let raw = ""; for await (const part of req) { raw += part; if (raw.length > 65536) throw Error(); }
    if (req.url === "/api/auth/oauth2/introspect") {
      const form = new URLSearchParams(raw), subject = form.get("token")?.replace("-fixture-token", "");
      if (!["alice", "bob", "eve"].includes(subject || "")) return json(200, { active: false });
      return json(200, { active: true, client_id: "database-fixture", sub: subject, sid: subject, exp: Math.floor(Date.now()/1000)+3600, webdock_role: "reader" });
    }
    const body = JSON.parse(raw), subject = String(body.accessToken || "").replace("-fixture-token", "");
    if (!["alice", "bob", "eve"].includes(subject)) return json(401, { error: "Fixture identity required" });
    if (req.url === "/api/studio") {
      if (body.operation === "session") return json(200, { data: { user: { id: subject, name: "Anna · Database preview", email: "anna@example.invalid", role: "user", emailVerified: true, twoFactorEnabled: false, mustChangePassword: false }, operator: false } });
      if (body.operation === "getTenant") return json(200, { data: { canManage: subject === "alice", customer: { id: "100", name: "Demo shop" } } });
      return json(400, { error: "Unknown fixture operation" });
    }
    if (req.url === "/api/databases/bridge") {
      const data = await run(db => executeDatabase(db, { subject, sessionID: subject, source: "studio", scopes: ["hosting:read", "hosting:write"] }, body.command));
      return json(200, { data });
    }
    json(404, {});
  } catch (error) { json(403, { error: error instanceof Error ? error.message : "Fixture request denied" }); }
});
const gateway = createGateway({ publicOrigin: "http://localhost:3137", allowedOrigins: ["http://localhost:3130"], runtimeOrigins: ["https://localhost:3139", "https://localhost:3159"],
  control: body => run(async db => {
    if (body.action === "exchange") return exchangeLaunch(db, body.code);
    if (body.action === "inspect") return inspectSession(db, body.token);
    return { ok: true };
  }),
});
function proxyRuntime(port: number, upstreamPort: number) {
const tls = httpsServer({ key: readFileSync("/tmp/webdock-tabularis-key.pem"), cert: readFileSync("/tmp/webdock-tabularis-cert.pem") }, (req, res) => {
  const upstream = httpRequest({ hostname: "127.0.0.1", port: upstreamPort, path: req.url, method: req.method, headers: req.headers }, response => { res.writeHead(response.statusCode || 502, response.headers); response.pipe(res); });
  upstream.on("error", () => { res.writeHead(502); res.end(); }); req.pipe(upstream);
});
const upgrades = new WebSocketServer({ server: tls });
upgrades.on("connection", (client, req) => {
  const headers: Record<string, string> = {};
  for (const name of ["host", "origin", "cookie", "x-tabularis-user", "x-tabularis-proxy-secret"]) { const value = req.headers[name]; if (typeof value === "string") headers[name] = value; }
  const upstream = new WebSocket("ws://127.0.0.1:"+upstreamPort+req.url, { headers });
  const pending: Buffer[] = [];
  client.on("message", data => upstream.readyState === WebSocket.OPEN ? upstream.send(data) : pending.push(Buffer.from(data.toString())));
  upstream.on("open", () => { for (const data of pending) upstream.send(data); });
  upstream.on("message", data => client.readyState === WebSocket.OPEN && client.send(data));
  upstream.on("error", () => client.close()); client.on("error", () => upstream.close());
  client.on("close", () => upstream.close()); upstream.on("close", () => client.close());
});
tls.listen(port, "127.0.0.1");
}
auth.listen(3136, "127.0.0.1"); gateway.listen(3137, "127.0.0.1"); proxyRuntime(3139, 3138); proxyRuntime(3159, 3158);
console.log("Local browser fixture: http://localhost:3136/fixture/login");
