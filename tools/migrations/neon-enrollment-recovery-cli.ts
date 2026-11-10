/** Run with Bun --no-env-file; owner custody is explicit and never a public HTTP fallback. */
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import { readPrivateInput } from "./operator-config";
import { inspectNeonEnrollmentRecovery, recoverNeonEnrollment, neonRecoveryConfig } from "./neon-enrollment-recovery";

let pool: Pool | undefined;
try {
  const [operation, target, path, ...extra] = process.argv.slice(2);
  if (!["inspect", "apply"].includes(operation ?? "") || !target || !path || extra.length)
    throw Error("invalid arguments");
  const config = neonRecoveryConfig(process.env, target);
  const input = await readPrivateInput(path);
  pool = new Pool({ ...(config.local ? { connectionString: config.url.href } : verifiedPostgresConfig(config.url.href, config.ca)), max: 1, connectionTimeoutMillis: 10000 });
  const db = await pool.connect();
  try {
    const run = operation === "inspect" ? inspectNeonEnrollmentRecovery : recoverNeonEnrollment;
    console.log(JSON.stringify(await run(db, config.database, config.providerOrigin, input), null, 2));
  } finally { db.release(); }
} catch {
  // Never print driver errors, input, connection strings or provider credential material.
  console.error("Offline Neon enrollment recovery failed. Use: bun --no-env-file tools/migrations/neon-enrollment-recovery-cli.ts inspect|apply host:port/database private-review.json. Verify owner custody, exact target, pinned origin and current review. If apply was interrupted, inspect the exact mapping/audit before retrying the same approved review.");
  process.exitCode = 1;
} finally { await pool?.end().catch(() => {}); }
