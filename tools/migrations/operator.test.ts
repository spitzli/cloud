import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtemp,
  writeFile,
  chmod,
  symlink,
  rm,
  readdir,
  readFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import {
  operatorConfig,
  operatorInput,
  readPrivateInput,
} from "./operator-config";
import {
  initialClient,
  initialPasswordHash,
  oauthSecretHash,
} from "./operator-provision";
const env = {
  DATABASE_URL_UNPOOLED:
    "postgresql://webdock_offline_migrate:private@127.0.0.1:5544/webdock",
  BETTER_AUTH_URL: "https://auth.example.test",
  WEBDOCK_STUDIO_ORIGIN: "https://studio.example.test",
  WEBDOCK_CONSOLE_ORIGIN: "https://console.example.test",
};
const profile = {
  email: "Owner@Example.test",
  name: "Initial operator",
  password: "private-initial-password",
  authOrigin: env.BETTER_AUTH_URL,
  studioOrigin: env.WEBDOCK_STUDIO_ORIGIN,
  consoleOrigin: env.WEBDOCK_CONSOLE_ORIGIN,
};
test("operator bootstrap requires exact target, dedicated offline role and matching canonical origins", () => {
  assert.equal(operatorConfig(env, "127.0.0.1:5544/webdock").local, true);
  assert.throws(() => operatorConfig(env, "elsewhere:5544/webdock"));
  assert.throws(() =>
    operatorConfig(
      {
        ...env,
        DATABASE_URL_UNPOOLED: env.DATABASE_URL_UNPOOLED.replace(
          "webdock_offline_migrate",
          "postgres",
        ),
      },
      "127.0.0.1:5544/webdock",
    ),
  );
  assert.throws(() =>
    operatorConfig(
      {
        ...env,
        DATABASE_URL_UNPOOLED: env.DATABASE_URL_UNPOOLED.replace(
          "127.0.0.1",
          "remote.example.test",
        ),
      },
      "remote.example.test:5544/webdock",
    ),
  );
  assert.equal(operatorInput(profile, env, true).email, "owner@example.test");
  assert.throws(() =>
    operatorInput(
      { ...profile, studioOrigin: "https://attacker.invalid" },
      env,
      true,
    ),
  );
  assert.throws(() =>
    operatorInput({ ...profile, operatorID: "123" }, env, true),
  );
  assert.throws(() =>
    operatorInput(
      profile,
      { ...env, WEBDOCK_INITIAL_OPERATOR_PASSWORD: "other-password-long" },
      true,
    ),
  );
  assert.equal(
    operatorInput(
      { ...profile, password: undefined },
      { ...env, WEBDOCK_INITIAL_OPERATOR_PASSWORD: "private-env-password" },
      true,
    ).password,
    "private-env-password",
  );
});
test("private input rejects public permissions and symlinks", async () => {
  const dir = await mkdtemp(join(tmpdir(), "operator-input-"));
  try {
    const file = join(dir, "input.json");
    await writeFile(file, JSON.stringify(profile), { mode: 0o600 });
    assert.deepEqual(await readPrivateInput(file), profile);
    await chmod(file, 0o644);
    await assert.rejects(readPrivateInput(file));
    await chmod(file, 0o600);
    await symlink(file, join(dir, "link"));
    await assert.rejects(readPrivateInput(join(dir, "link")));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test("password and OAuth hashing match the installed Better Auth SDK without creating auth sessions", async () => {
  const require = createRequire(
    new URL("../../legacy/control-plane/package.json", import.meta.url),
  );
  const crypto = await import(require.resolve("better-auth/crypto"));
  const password = await initialPasswordHash(profile.password);
  assert.notEqual(password, profile.password);
  assert.equal(
    await crypto.verifyPassword({ hash: password, password: profile.password }),
    true,
  );
  const dist = dirname(require.resolve("@better-auth/oauth-provider"));
  assert.equal(
    JSON.parse(await readFile(join(dist, "../package.json"), "utf8")).version,
    "1.7.7",
  );
  const util = (await readdir(dist)).find(
    (name) => name.startsWith("utils-") && name.endsWith(".mjs"),
  )!;
  const source = await readFile(join(dist, util), "utf8");
  const symbol = source.match(/storeClientSecret as (\w+)/)?.[1];
  assert.ok(symbol);
  const sdk = await import(pathToFileURL(join(dist, util)).href);
  assert.equal(
    await sdk[symbol]({}, {}, "fixture-client-secret"),
    oauthSecretHash("fixture-client-secret"),
  );
});
test("client specification records protocol callback separately from the Console origin and excludes passwords", () => {
  const input = operatorInput(profile, env, true),
    first = initialClient(input);
  assert.equal(
    first.specification.redirectURI,
    "https://studio.example.test/api/sso/callback",
  );
  assert.equal(
    first.specification.logoutURI,
    "https://studio.example.test/login",
  );
  assert.equal(first.specification.requirePKCE, true);
  assert.equal(
    first.checksum,
    initialClient({ ...input, password: "a-different-password" }).checksum,
  );
  assert.notEqual(
    first.checksum,
    initialClient({ ...input, name: "changed" }).checksum,
  );
  assert.ok(!JSON.stringify(first).includes(profile.password));
});
