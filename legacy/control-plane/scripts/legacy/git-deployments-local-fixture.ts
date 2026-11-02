// Local-only browser fixtures. Never use production credentials or send mail.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { parseEnv } from "node:util";
const root = path.resolve(import.meta.dirname, "../../..");
const scratch = path.join(root, ".superpowers/sdd/2026-10-10-git-deployments");
const u = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(u.hostname) ||
  u.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true" ||
  process.env.NODE_ENV === "production" ||
  !process.versions.node.startsWith("24.")
)
  throw Error("Disposable local DB required");
if (fs.existsSync(path.join(scratch, "browser-fixture.json")))
  throw Error("Fixture already exists; reuse it.");
const authOrigin = "http://localhost:3125",
  studioOrigin = "http://localhost:3120";
const authTest = parseEnv(
  fs.readFileSync(path.join(root, "apps/auth/.env.test.local"), "utf8"),
);
const adminTest = parseEnv(
  fs.readFileSync(path.join(root, "apps/console/.env.test.local"), "utf8"),
);
for (const value of [
  authTest.DATABASE_URL,
  adminTest.DATABASE_URL,
  adminTest.DATABASE_URL_UNPOOLED,
].filter(Boolean)) {
  const target = new URL(value!);
  if (
    !["localhost", "127.0.0.1"].includes(target.hostname) ||
    target.pathname !== "/webdock_api_test"
  )
    throw Error("Local test database required");
}
for (const key of Object.keys(process.env))
  if (/^(WEBDOCK_|VERCEL_|GITHUB_|SMTP_|AWS_)/.test(key))
    delete process.env[key];
Object.assign(process.env, {
  DATABASE_URL: authTest.DATABASE_URL,
  DATABASE_URL_UNPOOLED: authTest.DATABASE_URL,
  AUTH_MIGRATING: "",
  BETTER_AUTH_SECRET: authTest.BETTER_AUTH_SECRET,
  BETTER_AUTH_URL: authOrigin,
  NODE_ENV: "development",
  AUTH_TEST_MAIL: "true",
});
const { auth } = await import("../../src/modules/auth/auth");
const { database } = await import("../../src/db/runtime");
const { createIdentity, registerApplication } =
  await import("../../src/modules/auth/bootstrap");
const { migrateHosting } = await import("../../src/modules/hosting/schema");
const { hostingRegistryLockSQL } =
  await import("../../src/modules/hosting/registry-access");
const { managePlans, getPlans } = await import("../../src/modules/plans/plans");
const { gitDeploymentSchemaSQL } =
  await import("../../src/modules/hosting/git-deployment-schema");
const { gitEnvironmentSnapshot } =
  await import("../../src/modules/hosting/git-deployments");
function totp(uri: string) {
  const secret = new URL(uri).searchParams.get("secret")!;
  let bits = "";
  for (const char of secret.replace(/=/g, "").toUpperCase())
    bits += "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
      .indexOf(char)
      .toString(2)
      .padStart(5, "0");
  const key = Buffer.from(
      (bits.match(/.{8}/g) ?? []).map((b) => parseInt(b, 2)),
    ),
    count = Buffer.alloc(8);
  count.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const digest = crypto.createHmac("sha1", key).update(count).digest(),
    offset = digest[19] & 15;
  return String((digest.readUInt32BE(offset) & 0x7fffffff) % 1000000).padStart(
    6,
    "0",
  );
}
let stage = "initialize";
try {
  await auth.$context;
  await database.query(hostingRegistryLockSQL);
  await migrateHosting(database, studioOrigin + "/api/mcp");
  await database.query(gitDeploymentSchemaSQL);
  const password = crypto.randomBytes(24).toString("base64url");
  const rootUser = await createIdentity({
    email: `git-browser-${crypto.randomBytes(5).toString("hex")}@example.invalid`,
    name: "Local Git operator",
    password,
    operator: true,
    mustChangePassword: false,
  });
  const login = await auth.api.signInEmail({
    body: { email: rootUser.email, password },
    asResponse: true,
  });
  if (login.status !== 200) throw Error("Local login failed");
  const headers = new Headers({
    Cookie: login.headers
      .getSetCookie()
      .map((c) => c.split(";")[0])
      .join("; "),
  });
  stage = "enable-mfa";
  const mfa = await auth.api.enableTwoFactor({ headers, body: { password } });
  if (mfa.method !== "totp") throw Error("Local fixture requires TOTP");
  stage = "verify-mfa";
  const verified = await auth.api.verifyTOTP({
    headers,
    body: { code: totp(mfa.totpURI) },
    asResponse: true,
  });
  if (verified.status !== 200) throw Error("Local MFA setup failed");
  const updatedCookies = verified.headers
    .getSetCookie()
    .map((c) => c.split(";")[0]);
  if (updatedCookies.length) headers.set("Cookie", updatedCookies.join("; "));
  stage = "register-studio";
  const app = await registerApplication({
    label: "Git deployments local Studio",
    origin: studioOrigin,
    logoutPath: "/login",
    headers,
  });
  stage = "seed-customer";
  const customerPassword = crypto.randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `git-customer-${crypto.randomBytes(5).toString("hex")}@example.invalid`,
    name: "Local Git customer",
    password: customerPassword,
    mustChangePassword: false,
  });
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Git browser fixture') RETURNING id",
    )
  ).rows[0].id;
  const otherCustomerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Other hosting browser fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, user.id],
  );
  const projectID = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Git deployment demo',$1,'active') RETURNING id",
      [customerID],
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES(webdock_admin.next_snowflake(),$1,$2,'operator',$3)",
    [rootUser.name, rootUser.email, rootUser.id],
  );
  stage = "create-plan";
  const planName = "Git browser package " + customerID;
  await managePlans(headers, {
    action: "create-plan",
    name: planName,
    "hosting.apps": "5",
    "hosting.cpuMillicores": "2000",
    "hosting.memoryBytes": "4000000000",
    "hosting.volumeBytes": "10000000000",
    "hosting.ephemeralBytes": "1000000000",
    "hosting.replicasPerApp": "3",
    "hosting.concurrentDeployments": "2",
  });
  const plan = (await getPlans(headers)).find((p) => p.name === planName)!;
  await managePlans(headers, {
    action: "assign",
    customerID,
    planID: plan.id,
    revision: "0",
  });
  stage = "seed-git-data";
  const clusterID = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence,verified) VALUES('LOCAL fixture - no runtime','k3s','DE','local-only','Synthetic browser fixture, not real EU capacity',false) RETURNING id",
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode,own_images) VALUES($1,$2,'k3s',$3,$4,'selfservice',true)",
    [projectID, customerID, clusterID, "wd-" + projectID],
  );
  const spec = {
    template: "custom",
    image: "registry.example.invalid/demo@sha256:" + "1".repeat(64),
    cpuMillicores: 100,
    memoryBytes: 134217728,
    ephemeralBytes: 134217728,
    replicas: 1,
    port: 3000,
    healthPath: "/",
    args: [],
    volumeBytes: 0,
  };
  const targetID = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_app(project_id,name,spec,status,revision,observed_revision,observed_spec) VALUES($1,'Local fixture application',$2,'ready',1,1,$2) RETURNING id",
      [projectID, JSON.stringify(spec)],
    )
  ).rows[0].id;
  const installationID = (
    await database.query("SELECT webdock_auth.next_snowflake() AS id")
  ).rows[0].id;
  const repositoryID = (
    await database.query("SELECT webdock_auth.next_snowflake() AS id")
  ).rows[0].id;
  const connectionID = (
    await database.query(
      "INSERT INTO webdock_auth.git_connection(customer_id,installation_id,account_id,account_login,permissions) VALUES($1,$2,$3,'browser-fixture-local',$4) RETURNING id",
      [
        customerID,
        installationID,
        repositoryID,
        JSON.stringify({ metadata: "read", contents: "read", checks: "write" }),
      ],
    )
  ).rows[0].id;
  const metadata = {
    installationID,
    repositoryID,
    accountID: repositoryID,
    accountLogin: "browser-fixture-local",
    owner: "browser-fixture-local",
    name: "demo",
    fullName: "browser-fixture-local/demo",
    defaultBranch: "main",
    permissions: { metadata: "read", contents: "read", checks: "write" },
  };
  await database.query(
    "INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,$3,$4)",
    [connectionID, customerID, repositoryID, JSON.stringify(metadata)],
  );
  const source = (
    await database.query(
      "INSERT INTO webdock_auth.git_source(customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id) VALUES($1,$2,$3,$4,'main','.','dockerfile',$5) RETURNING *",
      [customerID, projectID, connectionID, repositoryID, targetID],
    )
  ).rows[0];
  const environment = await gitEnvironmentSnapshot(database, source);
  const buildIDs: string[] = [],
    releaseIDs: string[] = [];
  for (const [index, status] of [
    "succeeded",
    "failed",
    "succeeded",
  ].entries()) {
    const logs =
      index === 1
        ? 'npm ci\nType error: missing export buildSite\n<script>window.__gitLogXss = true</script>\n<img src=x onerror="window.__gitLogXss = true">'
        : `Local fixture build ${index + 1}\nBuild complete. Awaiting production approval.`;
    const build = (
      await database.query(
        "INSERT INTO webdock_auth.git_build(customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,status,subject,idempotency_key,logs,failure_code,generation,environment_identity,target_revision,finished_at,created_at) VALUES($1,$2,$3,$4,1,1,1,$5,$6,$7,$8,$9,1,$10,1,now(),now()-($11 * interval '1 minute')) RETURNING id",
        [
          customerID,
          projectID,
          source.id,
          String(index + 1).repeat(40),
          status,
          rootUser.id,
          "browser-" + source.id + "-" + index,
          logs,
          status === "failed" ? "BUILD_FAILED" : null,
          environment.identity,
          3 - index,
        ],
      )
    ).rows[0];
    buildIDs.push(build.id);
    if (status === "succeeded") {
      const digest = crypto
        .createHash("sha256")
        .update("local-artifact-" + source.id + index)
        .digest("hex");
      const artifact = (
        await database.query(
          "INSERT INTO webdock_auth.git_artifact(build_id,customer_id,project_id,kind,digest,storage_key,size_bytes) VALUES($1,$2,$3,'oci',$4,$5,10240) RETURNING id",
          [
            build.id,
            customerID,
            projectID,
            "sha256:" + digest,
            `${customerID}/${build.id}/1/${digest}.tar`,
          ],
        )
      ).rows[0];
      const release = (
        await database.query(
          "INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,status,source_revision,connection_generation,environment_revision,subject,environment_identity) VALUES($1,$2,$3,$4,1,1,1,$5,$6) RETURNING id",
          [
            customerID,
            projectID,
            artifact.id,
            index === 0 ? "failed" : "awaiting-approval",
            rootUser.id,
            environment.identity,
          ],
        )
      ).rows[0];
      releaseIDs.push(release.id);
    }
  }
  await database.query(
    "UPDATE webdock_auth.git_source SET latest_build_id=$2,environment_identity=$3 WHERE id=$1",
    [source.id, buildIDs.at(-1), environment.identity],
  );

  // Empty every local-env key before adding only known local fixture settings.
  const blank: Record<string, string> = {};
  for (const dir of [
    root,
    path.join(root, "apps/console"),
    path.join(root, "apps/auth"),
  ])
    for (const f of [
      ".env",
      ".env.local",
      ".env.development",
      ".env.development.local",
    ]) {
      const file = path.join(dir, f);
      if (fs.existsSync(file))
        for (const key of Object.keys(parseEnv(fs.readFileSync(file, "utf8"))))
          blank[key] = "";
    }
  const common = {
    ...blank,
    AUTH_TEST_MAIL: "true",
    NODE_ENV: "development",
    WEBDOCK_HOSTING_EU_VERIFIED: "",
    WEBDOCK_MCP_RESOURCE: studioOrigin + "/api/mcp",
  };
  const authEnv = {
    ...common,
    DATABASE_URL: authTest.DATABASE_URL,
    DATABASE_URL_UNPOOLED: authTest.DATABASE_URL,
    BETTER_AUTH_SECRET: authTest.BETTER_AUTH_SECRET,
    BETTER_AUTH_URL: authOrigin,
    WEBDOCK_STUDIO_CLIENT_ID: app.clientID,
    WEBDOCK_STUDIO_URL: studioOrigin,
    STUDIO_UI_ENABLED: "true",
    WEBDOCK_MCP_RESOURCE: studioOrigin + "/api/mcp",
  };
  const adminEnv = {
    ...common,
    DATABASE_URL: adminTest.DATABASE_URL,
    DATABASE_URL_UNPOOLED:
      adminTest.DATABASE_URL_UNPOOLED ?? adminTest.DATABASE_URL,
    PAYLOAD_SECRET: adminTest.PAYLOAD_SECRET,
    OPERATOR_EMAIL: rootUser.email,
    WEBDOCK_AUTH_ISSUER: authOrigin + "/api/auth",
    WEBDOCK_SSO_CLIENT_ID: app.clientID,
    WEBDOCK_SSO_CLIENT_SECRET: app.clientSecret,
    WEBDOCK_SSO_COOKIE_SECRET: crypto.randomBytes(40).toString("hex"),
    WEBDOCK_SSO_APP_ORIGIN: studioOrigin,
    WEBDOCK_SSO_ALLOW_LOCAL_HTTP: "true",
    NEXT_PUBLIC_SERVER_URL: studioOrigin,
  };
  fs.mkdirSync(scratch, { recursive: true, mode: 0o700 });
  for (const [name, env] of [
    ["auth", authEnv],
    ["console", adminEnv],
  ] as const)
    fs.writeFileSync(
      path.join(scratch, `${name}.env`),
      Object.entries(env)
        .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
        .join("\n") + "\n",
      { mode: 0o600, flag: "wx" },
    );
  fs.writeFileSync(
    path.join(scratch, "browser-fixture.json"),
    JSON.stringify({
      operator: {
        ...rootUser,
        password,
        totpURI: mfa.totpURI,
        backupCodes: mfa.backupCodes,
      },
      customer: { ...user, password: customerPassword },
      customerID,
      otherCustomerID,
      projectID,
      clusterID,
      targetID,
      connectionID,
      sourceID: source.id,
      buildIDs,
      releaseIDs,
      authOrigin,
      studioOrigin,
      clientID: app.clientID,
    }),
    { mode: 0o600, flag: "wx" },
  );
  await auth.api.signOut({ headers });
  console.log(
    JSON.stringify({
      customerID,
      projectID,
      targetID,
      buildIDs,
      releaseIDs,
      privateFixture: path.join(scratch, "browser-fixture.json"),
    }),
  );
} catch {
  throw Error(`Local fixture failed during ${stage}; no credentials logged.`);
} finally {
  await database.end();
}
