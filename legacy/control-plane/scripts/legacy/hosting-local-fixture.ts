import { hostingRegistryLockSQL } from "../../src/modules/hosting/registry-access";
// Local-only browser fixtures. Never use production credentials or send mail.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { parseEnv } from "node:util";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { migrateHosting } from "../../src/modules/hosting/schema";
import { managePlans, getPlans } from "../../src/modules/plans/plans";
const root = path.resolve(import.meta.dirname, "../../..");
const scratch = path.join(
  root,
  ".superpowers/sdd/2026-10-08-hosting-foundation",
);
const u = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(u.hostname) ||
  u.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
if (fs.existsSync(path.join(scratch, "browser-fixture.json")))
  throw Error("Fixture already exists; reuse it.");
const authOrigin = "http://localhost:3125",
  studioOrigin = "http://localhost:3120";
try {
  await auth.$context;
  await database.query(hostingRegistryLockSQL);
  await migrateHosting(database, studioOrigin + "/api/mcp");
  const password = crypto.randomBytes(24).toString("base64url");
  const rootUser = await createIdentity({
    email: `hosting-browser-${crypto.randomBytes(5).toString("hex")}@example.invalid`,
    name: "Local hosting operator",
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
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [rootUser.id],
  );
  const app = await registerApplication({
    label: "Hosting local Studio",
    origin: studioOrigin,
    logoutPath: "/login",
    headers,
  });
  const customerPassword = crypto.randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `hosting-customer-${crypto.randomBytes(5).toString("hex")}@example.invalid`,
    name: "Local hosting customer",
    password: customerPassword,
    mustChangePassword: false,
  });
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Hosting browser fixture') RETURNING id",
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
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Hosting test application',$1,'active') RETURNING id",
      [customerID],
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES(webdock_admin.next_snowflake(),$1,$2,'operator',$3)",
    [rootUser.name, rootUser.email, rootUser.id],
  );
  const planName = "Hosting browser package " + customerID;
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
    ...parseEnv(
      fs.readFileSync(path.join(root, "apps/auth/.env.test.local"), "utf8"),
    ),
    BETTER_AUTH_URL: authOrigin,
    WEBDOCK_STUDIO_CLIENT_ID: app.clientID,
    WEBDOCK_STUDIO_URL: studioOrigin,
    STUDIO_UI_ENABLED: "true",
    WEBDOCK_MCP_RESOURCE: studioOrigin + "/api/mcp",
  };
  const adminEnv = {
    ...common,
    ...parseEnv(
      fs.readFileSync(path.join(root, "apps/console/.env.test.local"), "utf8"),
    ),
    WEBDOCK_AUTH_ISSUER: authOrigin + "/api/auth",
    WEBDOCK_SSO_CLIENT_ID: app.clientID,
    WEBDOCK_SSO_CLIENT_SECRET: app.clientSecret,
    WEBDOCK_SSO_COOKIE_SECRET: crypto.randomBytes(40).toString("hex"),
    WEBDOCK_SSO_APP_ORIGIN: studioOrigin,
    WEBDOCK_SSO_ALLOW_LOCAL_HTTP: "true",
    NEXT_PUBLIC_SERVER_URL: studioOrigin,
  };
  fs.mkdirSync(scratch, { recursive: true });
  for (const [name, env] of [
    ["auth", authEnv],
    ["console", adminEnv],
  ] as const)
    fs.writeFileSync(
      path.join(scratch, `${name}.env`),
      Object.entries(env)
        .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
        .join("\n") + "\n",
      { mode: 0o600 },
    );
  fs.writeFileSync(
    path.join(scratch, "browser-fixture.json"),
    JSON.stringify({
      operator: { ...rootUser, password },
      customer: { ...user, password: customerPassword },
      customerID,
      otherCustomerID,
      projectID,
      clientID: app.clientID,
    }),
    { mode: 0o600 },
  );
  await auth.api.signOut({ headers });
  console.log(
    "Local hosting browser fixtures created; credentials saved privately.",
  );
} finally {
  await database.end();
}
