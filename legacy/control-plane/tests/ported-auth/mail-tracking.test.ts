import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import { platformSchemaSQL } from "../../src/modules/platform/platform";
import { mailSchemaSQL } from "../../src/modules/mail/tenant-mail";
import { senderDomainSchemaSQL } from "../../src/modules/mail/mail-domains";
import {
  getTenantTracking,
  manageTenantTracking,
  trackingHostname,
  trackingSchemaSQL,
} from "../../src/modules/mail/mail-tracking";
import { TrackingClient } from "../../src/modules/mail/turbo-tracking";
const origin = process.env.BETTER_AUTH_URL!;
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});
async function identity() {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `tracking-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Tracking test",
    password,
    mustChangePassword: false,
  });
  const response = await auth.handler(
    new Request(origin + "/api/auth/sign-in/email", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
      },
      body: JSON.stringify({ email: user.email, password }),
    }),
  );
  assert.equal(response.status, 200);
  return {
    ...user,
    headers: new Headers({
      Origin: origin,
      Cookie: response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    }),
  };
}
test("Tracking hostname accepts one label only", () => {
  assert.equal(trackingHostname("example.com"), "links.example.com");
  assert.equal(trackingHostname("example.com", " NEWS "), "news.example.com");
  for (const prefix of [
    "evil.com",
    "../x",
    "https://evil",
    "-bad",
    "bad-",
    "a".repeat(64),
  ])
    assert.throws(() => trackingHostname("example.com", prefix));
});
test("Tracking isolates sender/domain IDs, scopes child authorization and requires explicit toggles", async () => {
  for (const sql of [
    tenantSchemaSQL,
    accessSchemaSQL,
    platformSchemaSQL,
    mailSchemaSQL,
    senderDomainSchemaSQL,
    trackingSchemaSQL,
  ])
    await database.query(sql);
  const previous = (
    await database.query(
      "SELECT * FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const admin = await identity(),
    other = await identity(),
    prefix = randomBytes(6).toString("hex");
  const tenants = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Track A'),(webdock_auth.next_snowflake(),'Track B') RETURNING id",
    )
  ).rows;
  const [one, two] = tenants.map((t) => t.id);
  const mappings = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [[one, two]],
    )
  ).rows;
  for (const [tenant, user] of [
    [one, admin.id],
    [two, other.id],
  ])
    await database.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
      [mappings.find((m) => m.customer_id === tenant).organization_id, user],
    );
  const email = `${prefix}@example.invalid`,
    providerID = String(parseInt(prefix, 16)),
    senderName = `${prefix}.example.com`;
  await database.query(
    "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready','{\"active\":true}'),($4,$5,$6,'ready','{\"active\":true}')",
    [
      one,
      providerID,
      email,
      two,
      String(Number(providerID) + 1),
      `other-${email}`,
    ],
  );
  const sender = (
    await database.query(
      "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status,provider_id) VALUES($1,$2,'token','ownership_verified','22') RETURNING id",
      [one, senderName],
    )
  ).rows[0].id;
  let opening = false,
    exists = false,
    failCreate = false,
    calls = 0,
    forced = false,
    failVerify = false,
    failSetting = false;
  let onToolsRead: (() => Promise<void>) | undefined,
    onVerifyWrite: (() => Promise<void>) | undefined;
  const writes: string[] = [];
  const remote = {
    id: 73,
    domain_name: `links.${senderName}`,
    verification_domain: `verify.${senderName}`,
    verified: false,
    ssl: false,
    enabled: false,
    default: false,
  };
  const fetcher: typeof fetch = async (url, options) => {
    calls++;
    assert.equal(
      new Headers(options?.headers).get("Authorization"),
      "only-child-token",
    );
    assert.equal(new Headers(options?.headers).has("consumerKey"), false);
    const path = new URL(String(url)).pathname.replace("/api/v2", ""),
      method = options?.method || "GET";
    if (method !== "GET") writes.push(`${method} ${path}`);
    if (path === "/tools/link_branding" && method === "POST") {
      exists = true;
      assert.deepEqual(JSON.parse(String(options?.body)), {
        domain: senderName,
        subdomain: `links.${senderName}`,
      });
      if (failCreate) throw Error("uncertain secret");
      return Response.json({ id: 73 });
    }
    if (path === "/tools/openingTracking/enable" && failSetting)
      return Response.json({ status: "ERROR" });
    if (
      path === "/tools/openingTracking/disable" ||
      path === "/tools/link_branding/enabled/73/enable"
    )
      return new Response(null, { status: 204 });
    if (path === "/tools/openingTracking/enable") {
      opening = true;
      return new Response(null, { status: 204 });
    }
    if (path === "/tools/link_branding/verify/73") {
      remote.verified = true;
      if (onVerifyWrite) {
        const hook = onVerifyWrite;
        onVerifyWrite = undefined;
        await hook();
      }
      if (failVerify) throw Error("uncertain verify result");
      return Response.json({ message: "success" });
    }
    if (path === "/tools/link_branding" && method === "GET")
      return Response.json({
        count: exists ? 1 : 0,
        results: exists ? [remote] : [],
      });
    if (path === "/tools/link_branding/73") return Response.json(remote);
    if (path === "/tools/link_branding/domains_info")
      return Response.json({
        all_domains_disabled: true,
        match_sender: false,
        no_default_domain: true,
      });
    if (path === "/tools" && onToolsRead) {
      const hook = onToolsRead;
      onToolsRead = undefined;
      await hook();
    }
    if (path === "/tools")
      return Response.json([
        { id: "link_branding", enabled: false },
        { id: "clickTracking", enabled: false, forced },
        { id: "openingTracking", enabled: opening, forced },
      ]);
    if (["/tools/clickTracking", "/tools/openingTracking"].includes(path))
      return Response.json({
        id: path.split("/").at(-1),
        forced: false,
        enabled: path.includes("opening") && opening,
        settings: [],
      });
    throw Error(`Unexpected path ${path}`);
  };
  let authorizedEmail = "";
  const deps = {
    master: async () => ({
      getSubaccount: async (id: string | number) => {
        assert.equal(id, providerID);
        return { id: providerID, email, active: true };
      },
      authorizeSubaccount: async (address: string) => {
        authorizedEmail = address;
        return "only-child-token";
      },
    }),
    tracking: (token: string) => new TrackingClient(token, fetcher),
  };
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=EXCLUDED.settings",
    );
    await assert.rejects(getTenantTracking(admin.headers, two));
    await assert.rejects(
      manageTenantTracking(
        other.headers,
        two,
        { action: "create", senderDomainID: sender },
        deps,
      ),
      /belonging/,
    );
    assert.equal(calls, 0);
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "create", senderDomainID: sender, prefix: "evil.com" },
        deps,
      ),
    );
    assert.equal(calls, 0);
    await database.query(
      "INSERT INTO webdock_auth.mail_tracking_operation(customer_id,token,expires_at) VALUES($1,'busy',now()+interval '1 minute')",
      [one],
    );
    await assert.rejects(
      manageTenantTracking(admin.headers, one, { action: "refresh" }, deps),
      /in progress/,
    );
    assert.equal(calls, 0);
    await database.query(
      "DELETE FROM webdock_auth.mail_tracking_operation WHERE customer_id=$1",
      [one],
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation='busy',provider_operation_started_at=now() WHERE id=$1",
      [sender],
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "create", senderDomainID: sender },
        deps,
      ),
      /changing/,
    );
    assert.equal(writes.length, 0);
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation=NULL,provider_operation_started_at=NULL WHERE id=$1",
      [sender],
    );
    failCreate = true;
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "create", senderDomainID: sender, prefix: "links" },
        deps,
      ),
      /reconciliation/,
    );
    assert.equal(authorizedEmail, email);
    assert.deepEqual(writes, ["POST /tools/link_branding"]);
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "create", senderDomainID: sender, prefix: "links" },
        deps,
      ),
      /reserved/,
    );
    assert.equal(writes.length, 1);
    const pending = (await getTenantTracking(admin.headers, one)).domains[0];
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "cancel-reservation", domainID: pending.id, confirm: "yes" },
        deps,
      ),
      /older than 15/,
    );
    await database.query(
      "UPDATE webdock_auth.mail_tracking_domain SET updated_at=now()-interval '16 minutes' WHERE id=$1",
      [pending.id],
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "cancel-reservation", domainID: pending.id, confirm: "yes" },
        deps,
      ),
      /found and linked/,
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).domains[0].mapped,
      true,
    );
    failCreate = false;
    await manageTenantTracking(admin.headers, one, { action: "refresh" }, deps);
    const view = await getTenantTracking(admin.headers, one);
    assert.equal(view.domains.length, 1);
    assert.equal(view.domains[0].mapped, true);
    assert.equal(view.settings?.opening.enabled, false);
    const local = view.domains[0].id,
      before = calls;
    await assert.rejects(
      manageTenantTracking(
        other.headers,
        two,
        { action: "verify", domainID: local },
        deps,
      ),
      /belonging/,
    );
    assert.equal(calls, before);
    await manageTenantTracking(
      admin.headers,
      one,
      { action: "verify", domainID: local, providerID: "999" },
      deps,
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).domains[0].snapshot
        .verified,
      true,
    );
    await manageTenantTracking(
      admin.headers,
      one,
      { action: "setting", setting: "opening", value: "yes" },
      deps,
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).settings?.opening.enabled,
      true,
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "setting", setting: "opening", value: "no" },
        deps,
      ),
      /did not confirm/,
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).settings?.opening.enabled,
      true,
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "enabled", domainID: local, value: "yes" },
        deps,
      ),
      /did not confirm/,
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).domains[0].snapshot.enabled,
      false,
    );
    const unused = (
      await database.query(
        "INSERT INTO webdock_auth.mail_tracking_domain(customer_id,sender_domain_id,domain,updated_at) VALUES($1,$2,$3,now()-interval '16 minutes') RETURNING id",
        [one, sender, `unused.${senderName}`],
      )
    ).rows[0].id;
    await assert.rejects(
      manageTenantTracking(
        other.headers,
        two,
        { action: "cancel-reservation", domainID: unused, confirm: "yes" },
        deps,
      ),
      /unmapped reservation/,
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "cancel-reservation", domainID: unused },
        deps,
      ),
      /Confirm/,
    );
    const writeCount = writes.length;
    await manageTenantTracking(
      admin.headers,
      one,
      { action: "cancel-reservation", domainID: unused, confirm: "yes" },
      deps,
    );
    assert.equal(
      writes.length,
      writeCount,
      "Cancelling an absent reservation performs no provider writes",
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).domains.length,
      1,
    );
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "remove", domainID: local },
        deps,
      ),
      /Confirm/,
    );
    const externallyRemoved = (
      await database.query(
        "INSERT INTO webdock_auth.mail_tracking_domain(customer_id,sender_domain_id,domain,provider_id) VALUES($1,$2,$3,'74') RETURNING id",
        [one, sender, `gone.${senderName}`],
      )
    ).rows[0].id;
    const writesBeforeCleanup = writes.length;
    await manageTenantTracking(
      admin.headers,
      one,
      { action: "remove", domainID: externallyRemoved, confirm: "yes" },
      deps,
    );
    assert.equal(
      writes.length,
      writesBeforeCleanup,
      "Full scoped absence permits local cleanup without a provider write",
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).domains.length,
      1,
    );
    forced = true;
    const writesBeforePolicy = writes.length;
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "setting", setting: "click", value: "yes" },
        deps,
      ),
      /account policy/,
    );
    assert.equal(
      writes.length,
      writesBeforePolicy,
      "A forged direct action cannot bypass known forced policy",
    );
    forced = false;
    for (const change of [
      "revoke",
      "archive",
      "mailoff",
      "lease",
      "mapping",
      "inactive",
    ]) {
      onToolsRead = async () => {
        if (change === "revoke")
          await database.query(
            "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
            [admin.id],
          );
        if (change === "archive")
          await database.query(
            "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
            [one],
          );
        if (change === "mailoff")
          await database.query(
            "UPDATE webdock_auth.platform_settings SET settings='{}' WHERE id=true",
          );
        if (change === "lease")
          await database.query(
            "UPDATE webdock_auth.mail_tracking_operation SET token='replaced' WHERE customer_id=$1",
            [one],
          );
        if (change === "mapping")
          await database.query(
            "UPDATE webdock_auth.mail_tenant_account SET email=$2 WHERE customer_id=$1",
            [one, `changed-${email}`],
          );
        if (change === "inactive")
          await database.query(
            "UPDATE webdock_auth.mail_tenant_account SET snapshot='{\"active\":false}' WHERE customer_id=$1",
            [one],
          );
      };
      const writesBeforeRace: number = writes.length;
      await assert.rejects(
        manageTenantTracking(
          admin.headers,
          one,
          { action: "setting", setting: "opening", value: "yes" },
          deps,
        ),
      );
      assert.equal(
        writes.length,
        writesBeforeRace,
        `${change} during network I/O blocks the provider write`,
      );
      await database.query(
        "UPDATE webdock_auth.member SET role='admin' WHERE \"userId\"=$1",
        [admin.id],
      );
      await database.query(
        "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
        [one],
      );
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings='{\"mailEnabled\":true}' WHERE id=true",
      );
      await database.query(
        "UPDATE webdock_auth.mail_tenant_account SET email=$2,snapshot='{\"active\":true}' WHERE customer_id=$1",
        [one, email],
      );
      await database.query(
        "DELETE FROM webdock_auth.mail_tracking_operation WHERE customer_id=$1",
        [one],
      );
    }
    failVerify = true;
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "verify", domainID: local },
        deps,
      ),
    );
    assert.deepEqual(
      (await getTenantTracking(admin.headers, one)).domains[0].snapshot,
      {},
      "Uncertain verification clears prior green status",
    );
    failVerify = false;
    await manageTenantTracking(admin.headers, one, { action: "refresh" }, deps);
    failSetting = true;
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "setting", setting: "opening", value: "yes" },
        deps,
      ),
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).settings,
      undefined,
      "Uncertain settings writes clear the previous snapshot",
    );
    failSetting = false;
    onVerifyWrite = async () => {
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings='{}' WHERE id=true",
      );
    };
    await assert.rejects(
      manageTenantTracking(
        admin.headers,
        one,
        { action: "verify", domainID: local },
        deps,
      ),
      /changed/,
    );
    assert.deepEqual(
      (await getTenantTracking(admin.headers, one)).domains[0].snapshot,
      {},
      "State changed after the write cannot be persisted as success",
    );
    await database.query(
      "UPDATE webdock_auth.platform_settings SET settings='{\"mailEnabled\":true}' WHERE id=true",
    );
    await database.query(
      "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
      [admin.id],
    );
    assert.equal(
      (await getTenantTracking(admin.headers, one)).canManage,
      false,
    );
    await assert.rejects(
      manageTenantTracking(admin.headers, one, { action: "refresh" }, deps),
      /administrator/,
    );
    assert.ok(
      !JSON.stringify(await getTenantTracking(admin.headers, one)).includes(
        "only-child-token",
      ),
    );
  } finally {
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1,updated_by=$2,updated_at=$3 WHERE id=true",
        [previous.settings, previous.updated_by, previous.updated_at],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
