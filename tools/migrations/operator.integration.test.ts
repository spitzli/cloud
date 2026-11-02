import { test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { execFileSync, spawnSync, spawn } from "node:child_process";
import { mkdtemp, writeFile, readFile, stat, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { Pool } from "pg";
import { createServer } from "node:net";
import { once } from "node:events";
import { initializePluginState } from "../../legacy/control-plane/src/modules/plugins/bootstrap";
import { runtimeRoles } from "./config";
import { oauthSecretHash } from "./operator-provision";
import { startOperatorSetup } from "./operator-setup";
const root = fileURLToPath(new URL("../../", import.meta.url));
test(
  "initial operator bootstrap plus real MFA/PKCE and composed core API with all products disabled",
  { skip: process.env.WEBDOCK_TEST_OPERATOR !== "true", timeout: process.env.WEBDOCK_OPERATOR_BROWSER_HANDOFF ? 600000 : 180000 },
  async () => {
    const endpoint = execFileSync(
      "docker",
      ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
      { encoding: "utf8" },
    ).trim();
    assert.ok(endpoint.startsWith("unix://"));
    const dockerEnv: NodeJS.ProcessEnv = {
      ...process.env,
      DOCKER_HOST: endpoint,
    };
    delete dockerEnv.DOCKER_CONTEXT;
    const name = "webdock-operator-test-" + randomBytes(6).toString("hex"),
      password = randomBytes(32).toString("hex");
    const docker = (args: string[]) =>
      execFileSync("docker", args, { encoding: "utf8", env: dockerEnv }).trim();
    const dir = await mkdtemp(join(tmpdir(), "webdock-operator-"));
    docker([
      "run",
      "-d",
      "--name",
      name,
      "--label",
      "webdock.operator-fixture=" + name,
      "-e",
      "POSTGRES_PASSWORD=" + password,
      "-e",
      "POSTGRES_DB=webdock_operator_test",
      "-p",
      "127.0.0.1::5432",
      "postgres:18-alpine",
    ]);
    let owner: Pool | undefined;
    try {
      const port = docker(["port", name, "5432/tcp"]).split(":").at(-1)!;
      const url = new URL(
        `postgresql://postgres:${password}@127.0.0.1:${port}/webdock_operator_test`,
      );
      owner = new Pool({ connectionString: url.href, max: 1 });
      for (let n = 0; ; n++) {
        try {
          await owner.query("SELECT 1");
          break;
        } catch {
          if (n >= 60) throw Error("Fixture startup failed");
          await new Promise((r) => setTimeout(r, 250));
        }
      }
      const passwords = Object.fromEntries(
        [...runtimeRoles, "webdock_offline_migrate"].map((role) => [
          role,
          randomBytes(32).toString("hex"),
        ]),
      );
      const base = {
        PATH: process.env.PATH!,
        HOME: process.env.HOME!,
        DATABASE_URL_UNPOOLED: url.href,
        DATABASE_CONTEXT_SECRET: randomBytes(48).toString("hex"),
        BETTER_AUTH_URL: "https://auth.example.test",
        WEBDOCK_STUDIO_ORIGIN: "https://studio.example.test",
        WEBDOCK_CONSOLE_ORIGIN: "https://console.example.test",
      };
      const target = `127.0.0.1:${port}/webdock_operator_test`;
      const run = (
        script: string,
        args: string[],
        env: Record<string, string>,
      ) =>
        spawnSync(
          process.execPath,
          ["--no-env-file", "tools/migrations/" + script, target, ...args],
          {
            cwd: root,
            encoding: "utf8",
            env: { ...env, NODE_ENV: "development" },
          },
        );
      const install = run("install.ts", [], {
        ...base,
        WEBDOCK_BOOTSTRAP_EMPTY_DATABASE: "true",
        MIGRATION_ROLE_PASSWORDS: JSON.stringify(passwords),
      });
      assert.equal(install.status, 0, install.stdout + install.stderr);
      url.username = "webdock_offline_migrate";
      url.password = passwords.webdock_offline_migrate;
      const env = { ...base, DATABASE_URL_UNPOOLED: url.href },
        input = join(dir, "input.json"),
        output = join(dir, "secrets.json");
      const operatorPassword = randomBytes(24).toString("base64url");
      await writeFile(
        input,
        JSON.stringify({
          email: "operator@example.test",
          name: "Fixture operator",
          password: operatorPassword,
          authOrigin: base.BETTER_AUTH_URL,
          studioOrigin: base.WEBDOCK_STUDIO_ORIGIN,
          consoleOrigin: base.WEBDOCK_CONSOLE_ORIGIN,
        }),
        { mode: 0o600 },
      );
      // An existing unrelated user is preserved. Initial setup may not email-link an existing identity.
      await owner.query(
        `INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('Existing','operator@example.test',true,'user',false,false)`,
      );
      const collision = run("operator.ts", [input, output], env);
      assert.notEqual(collision.status, 0);
      assert.match(collision.stderr, /automatic linking is forbidden/);
      assert.equal(
        (
          await owner.query(
            'SELECT count(*)::int n FROM webdock_auth."oauthClient"',
          )
        ).rows[0].n,
        0,
      );
      await owner.query(
        `UPDATE webdock_auth."user" SET email='existing@example.test' WHERE email='operator@example.test'`,
      );
      // Exercise the real loopback wizard and shared offline transaction on the fresh database.
      const setup = await startOperatorSetup({ env, target, destination: output });
      let browserResult = "";
      try {
        const handoff = process.env.WEBDOCK_OPERATOR_BROWSER_HANDOFF;
        if (handoff) {
          // Opt-in local browser proof. Capabilities and the disposable password stay in a private file.
          await writeFile(handoff, JSON.stringify({ url: setup.url + "/#" + setup.capability, email: "operator@example.test", name: "Fixture operator", password: operatorPassword }), { mode: 0o600, flag: "wx" });
          console.log("Private disposable browser handoff is ready");
          try { assert.equal((await setup.closed).status, "success"); }
          finally { await rm(handoff, { force: true }); }
        } else {
          const response = await fetch(setup.url + "/setup", {
            method: "POST",
            headers: { Origin: setup.url, Authorization: "Bearer " + setup.capability, "Content-Type": "application/json" },
            body: JSON.stringify({ email: "operator@example.test", name: "Fixture operator", password: operatorPassword, ownershipConfirmed: true }),
          });
          browserResult = await response.text();
          assert.equal(response.status, 200, browserResult);
          assert.deepEqual(JSON.parse(browserResult), { created: true, securitySetupRequired: true });
          assert.equal((await setup.closed).status, "success");
        }
      } finally { await setup.close(); }
      const result = { stdout: browserResult, stderr: "" };
      const secrets = JSON.parse(await readFile(output, "utf8"));
      assert.equal((await stat(output)).mode & 0o777, 0o600);
      assert.ok(
        !result.stdout.includes(operatorPassword) &&
          !result.stderr.includes(operatorPassword),
      );
      assert.ok(
        !result.stdout.includes(secrets.environment.WEBDOCK_SSO_CLIENT_SECRET),
      );
      const user = (
        await owner.query(
          `SELECT u.*,a.password FROM webdock_auth."user" u JOIN webdock_auth.account a ON a."userId"=u.id WHERE u.id=$1`,
          [secrets.operatorID],
        )
      ).rows[0];
      assert.equal(user.role, "operator");
      assert.equal(user.twoFactorEnabled, false);
      assert.equal(user.mustChangePassword, true);
      const settings = (await owner.query("SELECT settings FROM webdock_auth.platform_settings WHERE id=true")).rows[0]?.settings;
      assert.equal(settings?.supportEmail, "operator@example.test", "Fresh support address belongs to the installation owner");
      assert.equal(settings?.cmsEnabled, false, "Removed CMS is not enabled by initial setup");
      const require = createRequire(
        new URL("../../legacy/control-plane/package.json", import.meta.url),
      );
      const crypto = await import(require.resolve("better-auth/crypto"));
      assert.equal(
        await crypto.verifyPassword({
          hash: user.password,
          password: operatorPassword,
        }),
        true,
      );
      assert.equal(
        (await owner.query("SELECT count(*)::int n FROM webdock_auth.session"))
          .rows[0].n,
        0,
      );
      assert.equal(
        (
          await owner.query(
            'SELECT count(*)::int n FROM webdock_auth."twoFactor"',
          )
        ).rows[0].n,
        0,
      );
      const client = (
        await owner.query(
          'SELECT * FROM webdock_auth."oauthClient" WHERE "clientId"=$1',
          [secrets.clientID],
        )
      ).rows[0];
      assert.equal(
        client.clientSecret,
        oauthSecretHash(secrets.environment.WEBDOCK_SSO_CLIENT_SECRET),
      );
      assert.equal(client.requirePKCE, true);
      assert.deepEqual(client.redirectUris, [
        "https://studio.example.test/api/sso/callback",
      ]);
      assert.equal(
        client.metadata.webdock_initial_setup.checksum,
        secrets.checksum,
      );
      assert.equal(
        (
          await owner.query(
            "SELECT auth_subject FROM webdock_admin.users WHERE id=$1",
            [secrets.portalUserID],
          )
        ).rows[0].auth_subject,
        secrets.operatorID,
      );
      assert.equal(
        (
          await owner.query(
            "SELECT count(*)::int n FROM webdock_auth.access_event WHERE action IN ('operator.bootstrap','console.bootstrap')",
          )
        ).rows[0].n,
        2,
      );
      const roleURL = (role: string) => {
        const value = new URL(url.href);
        value.username = role;
        value.password = passwords[role];
        return value.href;
      };
      // The admin core must function without the removed CMS inventory table.
      await owner.query("DROP TABLE webdock_admin.cms_instances CASCADE");
      const login = spawnSync(
        process.execPath,
        ["--no-env-file", "tools/migrations/operator-auth-fixture.ts"],
        {
          cwd: root,
          encoding: "utf8",
          env: {
            ...env,
            ...secrets.environment,
            WEBDOCK_TEST_OPERATOR: "true",
            NODE_ENV: "development",
            AUTH_TEST_MAIL: "true",
            BETTER_AUTH_SECRET: randomBytes(48).toString("hex"),
            DATABASE_URL: roleURL("webdock_auth_runtime"),
            AUTH_DATABASE_URL: roleURL("webdock_auth_bootstrap"),
            DATABASE_WORKER_URL: roleURL("webdock_worker_runtime"),
            REGISTRY_DATABASE_URL: roleURL("webdock_registry_runtime"),
            WEBDOCK_API_ORIGIN: "https://api.example.test",
            WEBDOCK_API_DOMAINS: "auth,portal,registry",
            WEBDOCK_MODULES: JSON.stringify({ configVersion: 1, modules: {} }),
            DATABASE_RLS_SECRET: base.DATABASE_CONTEXT_SECRET,
            OPERATOR_TEST_PASSWORD: operatorPassword,
            OPERATOR_TEST_SUBJECT: secrets.operatorID,
            OPERATOR_TEST_BINDING: secrets.bindingID,
          },
        },
      );
      assert.equal(login.status, 0, login.stdout + login.stderr);
      await initializePluginState(
        owner,
        { configVersion: 1, modules: {} },
        "fresh",
      );
      const inventoryOwner = new URL(url.href);
      inventoryOwner.username = "postgres";
      inventoryOwner.password = password;
      const inventory = spawnSync(process.execPath, ["--no-env-file", "tools/migrations/core-inventory-fixture.ts"], {
        cwd: root, encoding: "utf8", env: {
          ...env, ...secrets.environment, WEBDOCK_TEST_OPERATOR: "true", NODE_ENV: "development",
          BETTER_AUTH_SECRET: randomBytes(48).toString("hex"),
          CORE_INVENTORY_OWNER_URL: inventoryOwner.href,
          DATABASE_URL: roleURL("webdock_auth_runtime"), AUTH_DATABASE_URL: roleURL("webdock_auth_bootstrap"),
          DATABASE_WORKER_URL: roleURL("webdock_worker_runtime"), DATABASE_RLS_SECRET: base.DATABASE_CONTEXT_SECRET,
          WEBDOCK_MODULES: JSON.stringify({configVersion:1,modules:{}}),
        },
      });
      assert.equal(inventory.status, 0, inventory.stdout + inventory.stderr);
      console.log(inventory.stdout.trim());
      for (const provider of ["better-auth", "clerk"] as const) {
        const reservation = createServer();
        reservation.listen(0, "127.0.0.1");
        await once(reservation, "listening");
        const apiPort = (reservation.address() as { port: number }).port;
        await new Promise<void>((resolve, reject) =>
          reservation.close((error) => (error ? reject(error) : resolve())),
        );
        const ingressSecret = randomBytes(48).toString("hex");
        const runtimeEnv: NodeJS.ProcessEnv = {
          ...base,
          ...secrets.environment,
          // Production explicitly rejects loopback PostgreSQL without verified TLS.
          // Keep real scoped roles and signatures in this disposable local fixture.
          NODE_ENV: "development",
          PORT: String(apiPort),
          BIND_ADDRESS: "127.0.0.1",
          DATABASE_URL: roleURL("webdock_auth_runtime"),
          AUTH_DATABASE_URL: roleURL("webdock_auth_bootstrap"),
          DATABASE_WORKER_URL: roleURL("webdock_worker_runtime"),
          REGISTRY_DATABASE_URL: roleURL("webdock_registry_runtime"),
          DATABASE_POOL_MAX: "2",
          REGISTRY_POOL_MAX: "2",
          DATABASE_RLS_SECRET: base.DATABASE_CONTEXT_SECRET,
          WEBDOCK_API_ORIGIN: "https://api.example.test",
          WEBDOCK_API_DOMAINS: "auth,portal,registry",
          WEBDOCK_MODULES: JSON.stringify({ configVersion: 1, modules: {} }),
          WEBDOCK_INGRESS_SECRET: ingressSecret,
          WEBDOCK_TRUSTED_PROXIES: "127.0.0.1/32",
        };
        if (provider === "better-auth")
          runtimeEnv.BETTER_AUTH_SECRET = randomBytes(48).toString("hex");
        else {
          delete runtimeEnv.BETTER_AUTH_SECRET;
          runtimeEnv.WEBDOCK_AUTH_PROVIDER = JSON.stringify({
            provider: "clerk",
            issuer: "https://clerk.example.test",
            authorizedParties: [base.WEBDOCK_CONSOLE_ORIGIN],
            audience: "webdock-console",
          });
          runtimeEnv.WEBDOCK_OAUTH_PROVIDER = "disabled";
          runtimeEnv.CLERK_SECRET_KEY =
            "sk_test_" + randomBytes(24).toString("hex");
          runtimeEnv.WEBDOCK_CONSOLE_SERVICE_KEY =
            randomBytes(48).toString("hex");
        }
        const child = spawn(
          process.execPath,
          ["--no-env-file", "legacy/control-plane/src/core.ts"],
          {
            cwd: root,
            env: runtimeEnv,
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        let diagnostics = "";
        child.stdout.on("data", (data) => {
          diagnostics += String(data);
        });
        child.stderr.on("data", (data) => {
          diagnostics += String(data);
        });
        const redacted = () => {
          let output = diagnostics;
          for (const value of [
            ...Object.values(runtimeEnv),
            ...Object.values(passwords),
          ])
            if (value && value.length > 24)
              output = output.replaceAll(value, "[redacted]");
          return output.slice(-6000);
        };
        const request = (path: string, host = "api.example.test") =>
          fetch(`http://127.0.0.1:${apiPort}${path}`, {
            headers: {
              "x-forwarded-host": host,
              "x-forwarded-proto": "https",
              "x-forwarded-for": "127.0.0.1",
              "x-webdock-ingress-token": ingressSecret,
            },
            signal: AbortSignal.timeout(1000),
          });
        try {
          let ready = false;
          for (let attempt = 0; attempt < 150; attempt++) {
            assert.equal(
              child.exitCode,
              null,
              `${provider} process exited: ${redacted()}`,
            );
            try {
              ready = (await request("/readyz")).status === 200;
            } catch {}
            if (ready) break;
            await new Promise((resolve) => setTimeout(resolve, 200));
          }
          assert.ok(
            ready,
            `${provider} process did not become ready: ${redacted()}`,
          );
          assert.equal((await request("/healthz")).status, 200);
          for (const path of [
            "/v1/cms",
            "/api/cms",
            "/api/billing",
            "/v1/billing",
            "/api/hosting/bridge",
            "/api/databases/bridge",
            "/api/mail",
            "/api/storage",
          ])
            assert.equal(
              (await request(path)).status,
              404,
              `${provider} unexpectedly serves ${path}`,
            );
          if (provider === "clerk")
            assert.equal(
              (await request("/api/auth/get-session", "auth.example.test"))
                .status,
              404,
            );
        } finally {
          if (child.exitCode === null) {
            const stopped = once(child, "exit");
            child.kill("SIGTERM");
            const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
            try {
              await stopped;
            } finally {
              clearTimeout(timer);
            }
          }
        }
      }
      const repeat = run("operator.ts", [input, join(dir, "repeat.json")], env);
      assert.notEqual(repeat.status, 0);
      assert.match(repeat.stderr, /already exists/);
      assert.equal(
        (await owner.query('SELECT count(*)::int n FROM webdock_auth."user"'))
          .rows[0].n,
        2,
      );
    } finally {
      await owner?.end();
      assert.equal(
        docker([
          "inspect",
          "--format",
          '{{index .Config.Labels "webdock.operator-fixture"}}',
          name,
        ]),
        name,
      );
      docker(["rm", "-f", name]);
      await rm(dir, { recursive: true, force: true });
    }
  },
);
