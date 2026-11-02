// Disposable browser/control-plane fixture. No production URL or outgoing email is accepted.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { databaseSchemaSQL } from "../../src/modules/hosting/database-schema";
import { executeHosting } from "../../src/modules/hosting/service";
import {
  issueEnrollment,
  consumeEnrollment,
} from "../../src/modules/hosting/clusters";
import type { HostingActor } from "@webdock/hosting-contracts";
const root = path.resolve(import.meta.dirname, "../../..");
const scratch = path.join(
  root,
  ".superpowers/sdd/2026-10-12-managed-postgresql",
);
const target = new URL(process.env.DATABASE_URL!);
if (
  target.hostname !== "127.0.0.1" ||
  target.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true" ||
  process.env.NODE_ENV === "production" ||
  !process.versions.node.startsWith("24.")
)
  throw Error("Disposable Node 24 environment required");
if (fs.existsSync(path.join(scratch, "e2e-fixture.json")))
  throw Error("Reuse the existing fixture");
const authOrigin = "http://localhost:39345",
  studioOrigin = "http://localhost:39340";
function totp(uri: string) {
  let bits = "";
  for (const char of new URL(uri).searchParams
    .get("secret")!
    .replace(/=/g, "")
    .toUpperCase())
    bits += "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
      .indexOf(char)
      .toString(2)
      .padStart(5, "0");
  const key = Buffer.from(
      (bits.match(/.{8}/g) ?? []).map((b) => parseInt(b, 2)),
    ),
    counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const hash = crypto.createHmac("sha1", key).update(counter).digest(),
    offset = hash[19] & 15;
  return String((hash.readUInt32BE(offset) & 0x7fffffff) % 1000000).padStart(
    6,
    "0",
  );
}
function save(name: string, value: unknown) {
  fs.writeFileSync(path.join(scratch, name), JSON.stringify(value, null, 2), {
    mode: 0o600,
    flag: "wx",
  });
}
let stage = "schema";
try {
  await database.query(databaseSchemaSQL);
  await auth.$context;
  stage = "operator";
  const password = crypto.randomBytes(24).toString("base64url");
  const operator = await createIdentity({
    email: "postgres-operator@example.invalid",
    name: "PostgreSQL local operator",
    password,
    operator: true,
    mustChangePassword: false,
  });
  const login = await auth.api.signInEmail({
    body: { email: operator.email, password },
    asResponse: true,
  });
  if (login.status !== 200) throw Error("Login");
  const headers = new Headers({
    Cookie: login.headers
      .getSetCookie()
      .map((c) => c.split(";")[0])
      .join("; "),
  });
  const mfa = await auth.api.enableTwoFactor({ headers, body: { password } });
  if (mfa.method !== "totp") throw Error("TOTP required");
  const verified = await auth.api.verifyTOTP({
    headers,
    body: { code: totp(mfa.totpURI) },
    asResponse: true,
  });
  if (verified.status !== 200) throw Error("MFA");
  const cookies = verified.headers.getSetCookie().map((c) => c.split(";")[0]);
  if (cookies.length) headers.set("Cookie", cookies.join("; "));
  const session = await auth.api.getSession({ headers });
  if (!session) throw Error("Session");
  const actor: HostingActor = {
    subject: operator.id,
    sessionID: session.session.id,
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  };
  stage = "studio";
  const app = await registerApplication({
    label: "Managed PostgreSQL local Studio",
    origin: studioOrigin,
    logoutPath: "/login",
    headers,
  });
  await database.query(
    "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES(webdock_admin.next_snowflake(),$1,$2,'operator',$3)",
    [operator.name, operator.email, operator.id],
  );
  stage = "customers";
  const customers = [];
  for (const suffix of ["a", "b"]) {
    const customerPassword = crypto.randomBytes(24).toString("base64url");
    const user = await createIdentity({
      email: `postgres-customer-${suffix}@example.invalid`,
      name: `PostgreSQL customer ${suffix.toUpperCase()}`,
      password: customerPassword,
      mustChangePassword: false,
    });
    const customer = (
      await database.query(
        "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),$1) RETURNING id",
        [`PostgreSQL pilot ${suffix.toUpperCase()}`],
      )
    ).rows[0].id;
    const org = (
      await database.query(
        "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        [customer],
      )
    ).rows[0].organization_id;
    await database.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
      [org, user.id],
    );
    const project = (
      await database.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),$1,$2,'active') RETURNING id",
        [`PostgreSQL application ${suffix.toUpperCase()}`, customer],
      )
    ).rows[0].id;
    customers.push({
      id: customer,
      projectID: project,
      user: { id: user.id, email: user.email, password: customerPassword },
    });
  }
  stage = "enrollment";
  await database.query(
    "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,verified) VALUES('456','Disposable PostgreSQL pilot','local','DE','local-only','Isolated Docker test node; not production EU capacity',false)",
  );
  const enrollment = await executeHosting(actor, {
    action: "clusters.enrollment",
    clusterID: "456",
  });
  const token = await issueEnrollment(actor, enrollment.enrollmentID);
  const connection = await consumeEnrollment("456", token.token);
  save("e2e-agent.json", {
    ...connection,
    endpoint: "https://172.17.0.1:39346",
    sequence: 0,
  });
  stage = "plans";
  const plans = [];
  for (const kind of ["shared", "dedicated"] as const) {
    const shared = kind === "shared";
    const plan = {
      kind,
      name: shared ? "Shared PostgreSQL" : "Dedicated PostgreSQL Starter",
      monthlyPriceCents: shared ? 590 : 1490,
      currency: "EUR" as const,
      postgresMajor: 18 as const,
      cpuMillicores: shared ? null : 500,
      memoryBytes: shared ? null : 1073741824,
      volumeBytes: shared ? null : 10737418240,
      logicalStorageAllowanceBytes: shared ? 1073741824 : null,
      connectionLimit: 10,
      terms:
        "Synthetic local test. Fixed monthly net price, including while paused. Backups and recovery are not configured.",
    };
    const row = await executeHosting(actor, {
      action: "databases.plans.create",
      plan,
      idempotencyKey: crypto.randomUUID(),
    });
    await executeHosting(actor, {
      action: "databases.plans.setAvailability",
      planRevisionID: row.id,
      availability: "public",
      revision: 1,
      idempotencyKey: crypto.randomUUID(),
    });
    plans.push(row.id);
  }
  const common = {
    AUTH_TEST_MAIL: "true",
    NODE_ENV: "development",
    DATABASE_URL: process.env.DATABASE_URL!,
    DATABASE_URL_UNPOOLED: process.env.DATABASE_URL!,
    WEBDOCK_MCP_RESOURCE: studioOrigin + "/api/mcp",
  };
  const environments = {
    auth: {
      ...common,
      BETTER_AUTH_SECRET: process.env.BETTER_AUTH_SECRET!,
      BETTER_AUTH_URL: authOrigin,
      WEBDOCK_STUDIO_CLIENT_ID: app.clientID,
      WEBDOCK_STUDIO_URL: studioOrigin,
      STUDIO_UI_ENABLED: "true",
      WEBDOCK_DATABASE_PUBLIC_ENABLED: "true",
      WEBDOCK_DATABASE_CLUSTER_ID: "456",
      WEBDOCK_DATABASE_DOMAIN: "example.invalid",
    },
    console: {
      ...common,
      PAYLOAD_SECRET: process.env.PAYLOAD_SECRET!,
      OPERATOR_EMAIL: operator.email,
      NEXT_PUBLIC_SERVER_URL: studioOrigin,
      WEBDOCK_AUTH_ISSUER: authOrigin + "/api/auth",
      WEBDOCK_SSO_CLIENT_ID: app.clientID,
      WEBDOCK_SSO_CLIENT_SECRET: app.clientSecret,
      WEBDOCK_SSO_COOKIE_SECRET: crypto.randomBytes(40).toString("hex"),
      WEBDOCK_SSO_APP_ORIGIN: studioOrigin,
      WEBDOCK_SSO_ALLOW_LOCAL_HTTP: "true",
    },
  };
  for (const [name, value] of Object.entries(environments))
    fs.writeFileSync(
      path.join(root, "apps", name, ".env.development.local"),
      Object.entries(value)
        .map(([k, v]) => k + "=" + JSON.stringify(v))
        .join("\n") + "\n",
      { mode: 0o600, flag: "wx" },
    );
  save("e2e-fixture.json", {
    operator: { ...operator, password, totpURI: mfa.totpURI },
    operatorCookie: headers.get("Cookie"),
    actor,
    customers,
    plans,
    authOrigin,
    studioOrigin,
    clusterID: "456",
  });
  console.log(
    JSON.stringify({
      clusterID: "456",
      customers: customers.map((c) => ({ id: c.id, projectID: c.projectID })),
      plans,
      credentials: "Saved only to private ignored files.",
    }),
  );
} catch {
  throw Error(
    "Local PostgreSQL fixture failed at " + stage + "; no secrets logged",
  );
} finally {
  await database.end();
}
