import { Pool } from "pg";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { access, readFile, writeFile, unlink } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
const metadata = root + ".api-fixture.json",
  envFile = root + ".env.test.local";
const mode = process.argv[2];
if (!["up", "down"].includes(mode || "")) throw Error("Use fixture.ts up|down");
const context = process.env.DOCKER_CONTEXT;
const endpoint = context
  ? execFileSync(
      "docker",
      ["context", "inspect", context, "--format", "{{.Endpoints.docker.Host}}"],
      { encoding: "utf8" },
    ).trim()
  : process.env.DOCKER_HOST ||
    execFileSync(
      "docker",
      ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
      { encoding: "utf8" },
    ).trim();
if (!endpoint.startsWith("unix://"))
  throw Error("Test fixtures require a local Docker engine");
const dockerEnv: NodeJS.ProcessEnv = { ...process.env, DOCKER_HOST: endpoint };
delete dockerEnv.DOCKER_CONTEXT;
const docker = (args: string[]) =>
  execFileSync("docker", args, { env: dockerEnv, encoding: "utf8" }).trim();
if (mode === "down") {
  const stored = JSON.parse(await readFile(metadata, "utf8")) as {
    name: string;
    token: string;
    envHash?: string;
    rlsFiles?: Record<string, string>;
  };
  if (
    !/^webdock-api-fixture-[a-f0-9]{12}$/.test(stored.name) ||
    docker([
      "inspect",
      "--format",
      '{{index .Config.Labels "webdock.api-fixture"}}',
      stored.name,
    ]) !== stored.token
  )
    throw Error("Fixture ownership mismatch");
  const current = await readFile(envFile, "utf8").catch(() => null);
  const unchanged =
    current !== null &&
    stored.envHash === createHash("sha256").update(current).digest("hex");
  for (const name of [
    ".rls-fixture.json",
    ".env.rls.test.local",
    ".rls-installer.env",
  ]) {
    const contents = await readFile(root + name, "utf8").catch(() => null);
    if (
      contents !== null &&
      stored.rlsFiles?.[name] ===
        createHash("sha256").update(contents).digest("hex")
    )
      await unlink(root + name);
  }
  docker(["rm", "-f", stored.name]);
  await unlink(metadata);
  if (unchanged) await unlink(envFile);
  console.log(
    unchanged
      ? "Removed this workspace’s disposable fixture."
      : "Removed disposable container; preserved unverified or modified local env file.",
  );
} else {
  if (
    (await access(envFile).then(
      () => true,
      () => false,
    )) ||
    (await access(metadata).then(
      () => true,
      () => false,
    ))
  )
    throw Error(
      "Existing fixture configuration preserved; use its explicit teardown first",
    );
  const token = randomBytes(6).toString("hex"),
    name = "webdock-api-fixture-" + token,
    password = randomBytes(24).toString("hex");
  docker([
    "run",
    "-d",
    "--name",
    name,
    "--label",
    "webdock.api-fixture=" + token,
    "-e",
    "POSTGRES_PASSWORD=" + password,
    "-e",
    "POSTGRES_DB=webdock_api_test",
    "-p",
    "127.0.0.1::5432",
    "postgres:18-alpine",
  ]);
  try {
    const port = docker(["port", name, "5432/tcp"]).split(":").at(-1)!;
    const url = `postgresql://postgres:${password}@127.0.0.1:${port}/webdock_api_test`;
    const probe = new Pool({
      connectionString: url,
      max: 1,
      connectionTimeoutMillis: 1000,
    });
    try {
      for (let n = 0; n < 60; n++) {
        try {
          await probe.query("SELECT 1");
          break;
        } catch {
          if (n === 59) {
            console.error(docker(["logs", "--tail", "15", name]));
            throw Error("Fixture database did not start");
          }
          await Bun.sleep(500);
        }
      }
    } finally {
      await probe.end();
    }
    const env: NodeJS.ProcessEnv = {
      PATH: process.env.PATH!,
      HOME: process.env.HOME!,
      DATABASE_URL: url,
      DATABASE_URL_UNPOOLED: url,
      API_TEST_DATABASE_URL: url,
      AUTH_TEST_MAIL: "true",
      NODE_ENV: "development",
      BETTER_AUTH_URL: "http://localhost:3125",
      BETTER_AUTH_SECRET: randomBytes(32).toString("hex"),
      PAYLOAD_SECRET: randomBytes(32).toString("hex"),
    };
    const result = spawnSync(
      process.execPath,
      ["--no-env-file", "scripts/legacy/setup-mail-test.ts"],
      { cwd: root, env, stdio: "inherit" },
    );
    if (result.status !== 0) throw Error("Fixture schema setup failed");
    const contents =
      Object.entries(env)
        .filter(([key]) => !["PATH", "HOME"].includes(key))
        .map(([key, value]) => key + "=" + value)
        .join("\n") + "\n";
    await writeFile(envFile, contents, { mode: 0o600, flag: "wx" });
    await writeFile(
      metadata,
      JSON.stringify({
        name,
        token,
        envHash: createHash("sha256").update(contents).digest("hex"),
      }),
      { mode: 0o600, flag: "wx" },
    );
    console.log(
      "Isolated API fixture ready; credentials saved only in ignored local files.",
    );
  } catch (error) {
    docker(["rm", "-f", name]);
    throw error;
  }
}
