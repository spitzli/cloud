import { readFile, readdir } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { testEnvironment, rlsEnvironment } from "./test-environment";
const root = fileURLToPath(new URL("../", import.meta.url));
const mode = process.argv[2] || "all";
if (!["unit", "integration", "ported", "rls", "all"].includes(mode))
  throw Error("Unknown test selection");
function run(
  command: string,
  args: string[],
  env = testEnvironment({ ...process.env, API_TEST_DATABASE_URL: undefined }),
) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env });
  if (result.status !== 0) process.exit(result.status || 1);
}
const files = (await readdir(root + "tests")).filter((name) =>
  name.endsWith(".test.ts"),
);
const integration = new Set([
  "clerk-oauth-session.integration.test.ts",
  "clerk-projection.integration.test.ts",
  "auth-adapter.test.ts",
  "mail-origin.integration.test.ts",
  "auth-protocol.test.ts",
  "database-compat.test.ts",
  "native-ui.integration.test.ts",
  "cms-bridge.integration.test.ts",
]);
const rls = new Set([
  "database-job-identity-rls.integration.test.ts",
  "retained-billing-schema.integration.test.ts",
  "plugins-startup-rls.integration.test.ts",
  "plugins-runtime-state-rls.integration.test.ts",
  "plugins-state-rls.integration.test.ts",
  "plugins-admission.integration.test.ts",
  "server.integration.test.ts",
  "clerk-worker-rls.integration.test.ts",
  "module-preflight-rls.integration.test.ts",
  "module-preflight.integration.test.ts",
  "clerk-rls.integration.test.ts",
  "clerk-worker.integration.test.ts",
  "clerk-mail.integration.test.ts",
  "service-bindings-rls.integration.test.ts",
  "project-setup-worker.integration.test.ts",
  "jobs.integration.test.ts",
  "registry.integration.test.ts",
  "registry-cache.integration.test.ts",
  "native-budget-rls.integration.test.ts",
  "mcp.integration.test.ts",
  "vercel-events.integration.test.ts",
  "worker-profile.integration.test.ts",
]);
const requested = process.argv.slice(3);
if (mode === "unit" || mode === "all")
  run(process.execPath, [
    "--no-env-file",
    "test",
    "--preload",
    "./tests/helpers/no-network.ts",
    ...files
      .filter((name) => !integration.has(name) && !rls.has(name))
      .map((name) => "tests/" + name),
  ]);
if (["integration", "ported", "all"].includes(mode)) {
  const url = process.env.API_TEST_DATABASE_URL;
  if (!url)
    throw Error(
      "API_TEST_DATABASE_URL is required; create an isolated test fixture first",
    );
  const env = testEnvironment(process.env);
  if (mode === "integration" || mode === "all") {
    // Native Auth tests own persistent fixture identities; each file gets its own module lifecycle.
    for (const name of requested.length ? requested : [...integration])
      run(
        process.execPath,
        [
          "--no-env-file",
          "test",
          "--preload",
          "./tests/helpers/no-network.ts",
          name.startsWith("tests/") ? name : "tests/" + name,
        ],
        env,
      );
  }
  if (mode === "ported" || mode === "all") {
    const ported = (await readdir(root + "tests/ported-auth"))
      .filter((name) => name.endsWith(".test.ts"))
      .map((name) => "tests/ported-auth/" + name);
    run(
      process.execPath,
      [
        "--no-env-file",
        "test",
        // Like node --test, every file gets its own module state (shared pools are ended per file).
        "--isolate",
        "--preload",
        "./tests/helpers/no-network.ts",
        ...ported,
        ...(await readdir(root + "tests/ported-admin"))
          .filter((name) => name.endsWith(".test.ts"))
          .map((name) => "tests/ported-admin/" + name),
      ],
      env,
    );
  }
}
if (mode === "rls") {
  if (!process.env.WEBDOCK_RLS_FIXTURE_FILE)
    throw Error("Explicit restricted-role fixture required");
  const fixture = JSON.parse(
    await readFile(process.env.WEBDOCK_RLS_FIXTURE_FILE, "utf8"),
  );
  const env = rlsEnvironment(
    process.env,
    fixture,
    process.env.API_TEST_OWNER_URL || "",
  );
  const selectedRls=requested.length?requested.map(name=>name.replace(/^tests\//,"")):[...rls];
  if(selectedRls.some(name=>!rls.has(name)))throw Error("Unknown RLS test selection");
  for (const name of selectedRls)
    run(
      process.execPath,
      [
        "--no-env-file",
        "test",
        "--preload",
        "./tests/helpers/no-network.ts",
        "tests/" + name,
      ],
      env,
    );
  if(requested.length)process.exit(0);
  const names = [
    "database-rls-postgres",
    "database-gateway-rls",
    "database-orders-rls",
    "mail-rls-postgres",
  ];
  run(
    process.execPath,
    [
      "--no-env-file",
      "test",
      "--isolate",
      "--preload",
      "./tests/helpers/no-network.ts",
      ...names.map((name) => "tests/ported-auth/" + name + ".test.ts"),
    ],
    { ...env, DATABASE_URL: env.API_TEST_OWNER_URL },
  );
}
