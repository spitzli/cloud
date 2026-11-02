/** Source-derived schema only, from this worktree's freshly created disposable fixture. */
import { readFile, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
const root = new URL("../../", import.meta.url);
const metadata = JSON.parse(
  await readFile(new URL("legacy/control-plane/.api-fixture.json", root), "utf8"),
);
if (!/^webdock-api-fixture-[a-f0-9]{12}$/.test(metadata.name))
  throw Error("Owned local fixture required");
const endpoint = execFileSync(
  "docker",
  ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
  { encoding: "utf8" },
).trim();
if (!endpoint.startsWith("unix://"))
  throw Error("Local Docker engine required");
const env: NodeJS.ProcessEnv = { ...process.env, DOCKER_HOST: endpoint };
delete env.DOCKER_CONTEXT;
const label = execFileSync(
  "docker",
  [
    "inspect",
    "--format",
    '{{index .Config.Labels "webdock.api-fixture"}}',
    metadata.name,
  ],
  { encoding: "utf8", env },
).trim();
if (label !== metadata.token) throw Error("Fixture ownership mismatch");
let sql = execFileSync(
  "docker",
  [
    "exec",
    metadata.name,
    "pg_dump",
    "-U",
    "postgres",
    "-d",
    "webdock_api_test",
    "--schema-only",
    "--no-owner",
    "--no-privileges",
    "--schema=webdock_auth",
    "--schema=webdock_admin",
    "--schema=webdock_mail",
  ],
  { encoding: "utf8", env, maxBuffer: 16 * 1024 * 1024 },
);
// pg_dump's random psql restriction token is not SQL and is immaterial to schema identity.
sql = sql
  .split("\n")
  .filter(
    (line) =>
      !line.startsWith("\\restrict ") &&
      !line.startsWith("\\unrestrict ") &&
      !line.startsWith("-- Dumped"),
  )
  .join("\n");
if (/^COPY .* FROM stdin;/m.test(sql))
  throw Error("Baseline must contain no data");
await writeFile(new URL("tools/migrations/baseline.sql", root), sql);
await writeFile(
  new URL("tools/migrations/baseline.sha256", root),
  createHash("sha256").update(sql).digest("hex") + "\n",
);
console.log("Wrote schema-only baseline from the owned fixture.");
