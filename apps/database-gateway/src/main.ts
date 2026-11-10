import { runtimeOrigin } from "@spitzli/database-contracts";
import { createManagedRuntimeManager } from "./managed-runtimes";
import { createGateway } from "./server";

const required = (name: string) => { const value = process.env[name]; if (!value) throw Error(`${name} is required`); return value; };
const authOrigin = runtimeOrigin.parse(required("SPITZLI_AUTH_ORIGIN"));
const secret = required("SPITZLI_DATABASE_GATEWAY_SECRET");
if (secret.length < 32) throw Error("Gateway secret must have at least 32 characters");
const managedRuntimes = process.env.SPITZLI_DATABASE_MANAGED_RUNTIMES === "true" ? createManagedRuntimeManager({
  executable: process.env.TABULARIS_EXECUTABLE || "/usr/local/bin/tabularis",
  webRoot: process.env.TABULARIS_WEB_ROOT || "/opt/tabularis/web",
  temporaryDirectory: process.env.SPITZLI_DATABASE_RUNTIME_DIRECTORY || "/tmp",
  capacity: Number(process.env.SPITZLI_DATABASE_RUNTIME_CAPACITY || 8),
}) : undefined;
const server = createGateway({
  managedRuntimes,
  publicOrigin: required("SPITZLI_DATABASE_GATEWAY_ORIGIN"),
  allowedOrigins: required("SPITZLI_DATABASE_BROWSER_ORIGINS").split(","),
  runtimeOrigins: (process.env.SPITZLI_DATABASE_RUNTIME_ORIGINS || "").split(",").filter(Boolean),
  expectedCommit: required("TABULARIS_EXPECTED_COMMIT"),
  audit: event => console.info(JSON.stringify({ event: "database.rpc", ...event })),
  control: async body => {
    const response = await fetch(authOrigin+"/api/databases/gateway", { method: "POST", redirect: "error", signal: AbortSignal.timeout(10_000),
      headers: { "Content-Type": "application/json", Authorization: "Bearer "+secret }, body: JSON.stringify(body) });
    if (!response.ok) throw Error("Database authorization unavailable");
    const result = await response.json(); return result.data;
  },
});
server.listen(Number(process.env.PORT || 3137), process.env.HOST || "127.0.0.1", () => console.log("Database gateway listening"));
for (const signal of ["SIGTERM", "SIGINT"] as const) process.on(signal, () => { void server.shutdown(); });
