import { Pool } from "pg";
import { setTimeout } from "node:timers/promises";
import { DockerMailRuntime } from "./runtime/docker.ts";
import { processMailOperation } from "./provisioning.ts";

const required = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`Configure ${name} before starting the Mail worker`);
  return value;
};
if (process.env.MAIL_RUNTIME_BACKEND !== "local-docker" || process.env.AUTH_TEST_MAIL !== "true")
  throw new Error("This worker is a local integration adapter; managed Mail uses the Webdock hosting agent");
const pool = new Pool({ connectionString: required("DATABASE_URL"), max: 4, connectionTimeoutMillis: 10_000 });
let stop = false;
process.once("SIGINT", () => { stop = true; });
process.once("SIGTERM", () => { stop = true; });
try {
  const hostID = required("MAIL_HOST_ID"), key = required("MAIL_ENCRYPTION_KEY");
  const runtime = new DockerMailRuntime({ socket: required("MAIL_DOCKER_SOCKET"), namespace: required("MAIL_RUNTIME_NAMESPACE"), hostnameSuffix: required("MAIL_HOSTNAME_SUFFIX") });
  do {
    const processed = await processMailOperation(pool, runtime, hostID, key);
    if (process.argv.includes("--once")) break;
    if (!processed && !stop) await setTimeout(2000);
  } while (!stop);
} catch {
  console.error("Mail worker could not complete its operation. Check host configuration, database access and pending review states.");
  process.exitCode = 1;
} finally { await pool.end(); }
