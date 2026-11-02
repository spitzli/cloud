import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { DockerMailRuntime, type InstanceCredentials } from "../../src/runtime/docker.ts";
import { StalwartClient } from "../../src/stalwart/client.ts";

const project = process.env.MAIL_TEST_PROJECT!;
assert.match(project || "", /^webdock-mail-test-[a-z0-9-]+$/);
const name = `${project}-mail-9001`;
const operation = { id: "1", customerID: "9001", revision: "1", enabled: true, instanceKey: "mail-9001", hostID: "test", leaseToken: "test" };
const docker = (...args: string[]) => execFileSync("docker", args, { encoding: "utf8", timeout: 30_000 });
test.after(() => {
  for (const args of [["rm", "-f", name], ["network", "rm", name], ["volume", "rm", `${name}-config`, `${name}-data`]]) {
    try { docker(...args); } catch { /* The failing test may not have created a resource. */ }
  }
});

test("provisioning creates one CE instance, removes recovery access, and resumes retained data", async () => {
  const runtime = new DockerMailRuntime({ socket: "unix:///var/run/docker.sock", namespace: project, hostnameSuffix: "mail.test" });
  let credentials: InstanceCredentials = { bootstrapPassword: randomBytes(32).toString("base64url") };
  const save = async (next: InstanceCredentials) => { credentials = next; };
  const first = await runtime.provision(operation, credentials, save);
  assert.equal(first.publicURL, "https://mail-9001.mail.test");
  assert.ok(credentials.username && credentials.password);
  assert.equal(credentials.bootstrapPassword, undefined);
  const admin = new StalwartClient({ url: first.internalURL, authorization: `Basic ${Buffer.from(`${credentials.username}:${credentials.password}`).toString("base64")}`, allowInsecureHttp: true });
  const domains = await admin.call("x:Domain/get", { ids: null });
  assert.ok((domains.list as unknown[]).length);
  const [container] = JSON.parse(docker("inspect", name));
  assert.ok(!container.Config.Env.some((value: string) => value.startsWith("STALWART_RECOVERY_ADMIN=")));
  assert.deepEqual(container.HostConfig.PortBindings || {}, {});
  const before = container.Mounts.map((mount: { Name: string }) => mount.Name).sort();
  await runtime.suspend({ ...operation, enabled: false });
  assert.equal(JSON.parse(docker("inspect", name))[0].State.Running, false);
  const second = await runtime.provision(operation, credentials, save);
  const [resumed] = JSON.parse(docker("inspect", name));
  assert.equal(resumed.State.Running, true);
  assert.deepEqual(resumed.Mounts.map((mount: { Name: string }) => mount.Name).sort(), before);
  const check = new StalwartClient({ url: second.internalURL, authorization: `Basic ${Buffer.from(`${credentials.username}:${credentials.password}`).toString("base64")}`, allowInsecureHttp: true });
  assert.deepEqual((await check.call("x:Domain/get", { ids: null })).list, domains.list);
  await runtime.suspend({ ...operation, enabled: false });
  docker("rm", name);
  docker("volume", "rm", `${name}-config`);
  await assert.rejects(runtime.provision(operation, credentials, save), /volume.*missing/i);
  assert.throws(() => docker("volume", "inspect", `${name}-config`), "must not silently replace lost configuration with an empty store");
});
