import { execFileSync, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";

if (Number(process.versions.node.split(".")[0]) < 24 || process.platform !== "linux") {
  throw new Error("Mail integration tests require Node 24+ and a local Linux Docker engine.");
}
const selectedContext = process.env.DOCKER_CONTEXT;
const dockerHost = selectedContext
  ? execFileSync("docker", ["context", "inspect", selectedContext, "--format", "{{.Endpoints.docker.Host}}"], { encoding: "utf8" }).trim()
  : process.env.DOCKER_HOST || execFileSync("docker", ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"], { encoding: "utf8" }).trim();
if (!dockerHost.startsWith("unix://")) throw new Error("Mail tests refuse remote Docker engines.");
// DOCKER_CONTEXT takes precedence over DOCKER_HOST; pin the validated daemon for every child.
const dockerEnv = { ...process.env, DOCKER_HOST: dockerHost };
delete dockerEnv.DOCKER_CONTEXT;
const root = fileURLToPath(new URL("../", import.meta.url));
const project = `webdock-mail-test-${randomBytes(6).toString("hex")}`;
const args = ["compose", "-p", project, "-f", "infra/mail/compose.test.yml"];
let cleaned = false;
function cleanup() {
  if (cleaned) return;
  cleaned = true;
  execFileSync("docker", [...args, "down", "--volumes", "--remove-orphans"], { cwd: root, env: dockerEnv, stdio: "inherit", timeout: 60_000 });
}
process.once("SIGINT", () => { cleanup(); process.exit(130); });
process.once("SIGTERM", () => { cleanup(); process.exit(143); });
try {
  console.log(`Starting isolated CE lab ${project} (no published ports, no internet egress).`);
  execFileSync("docker", [...args, "up", "-d"], { cwd: root, env: dockerEnv, stdio: "inherit", timeout: 60_000 });
  const result = spawnSync(process.execPath, ["test", "./legacy/mail/tests/integration/"], {
    cwd: root, env: { ...dockerEnv, MAIL_TEST_PROJECT: project }, stdio: "inherit", timeout: 120_000,
  });
  process.exitCode = result.status ?? 1;
  if (result.error) console.error(result.error.message);
} finally { cleanup(); }
