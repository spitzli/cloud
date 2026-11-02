import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes, createHash } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import {
  acceptOffer,
  getOffer,
  getPlans,
  getTenantPlan,
  managePlans,
  plansSchemaSQL,
} from "../../src/modules/plans/plans";
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
async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `plans-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Plan test",
    password,
    operator,
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
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
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
test("Plans preserve snapshots and extras, serialize revisions, and require current tenant administration for private offers", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(plansSchemaSQL);
  await database.query(plansSchemaSQL);
  const root = await identity(true),
    admin = await identity(),
    other = await identity(),
    member = await identity();
  const customerIDs: string[] = [],
    organizationIDs: string[] = [];
  try {
    const customers = (
      await database.query(
        "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Plans A'),(webdock_auth.next_snowflake(),'Plans B') RETURNING id",
      )
    ).rows;
    const [one, two] = customers.map((c) => c.id as string);
    customerIDs.push(one, two);
    const mappings = (
      await database.query(
        "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
        [[one, two]],
      )
    ).rows;
    organizationIDs.push(...mappings.map((mapping) => mapping.organization_id));
    const org = (customer: string) =>
      mappings.find((m) => m.customer_id === customer)!.organization_id;
    await database.query(
      "INSERT INTO webdock_auth.member(\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,'admin',now()),($3,$4,'admin',now()),($1,$5,'member',now())",
      [org(one), admin.id, org(two), other.id, member.id],
    );
    const input = {
      name: "Base " + randomBytes(5).toString("hex"),
      description: "Snapshot",
      storageMB: "500",
      mailMessages: "1000",
      transferGB: "2",
      websites: "1",
      editors: "2",
    };
    const manage = (data: Record<string, string>) =>
      managePlans(root.headers, { customerID: one, ...data });
    await assert.rejects(getPlans(admin.headers), /operator/);
    await assert.rejects(getTenantPlan(admin.headers, two));
    await assert.rejects(
      managePlans(admin.headers, { action: "create-plan", ...input }),
      /operator/,
    );
    for (const storageMB of [
      "-1",
      "Infinity",
      "9007199254740991",
      "1e6",
      "0.0000001",
    ])
      await assert.rejects(
        manage({ action: "create-plan", ...input, storageMB }),
        /Allowances/,
      );
    for (const editors of ["1.5", "9007199254740991.1"])
      await assert.rejects(
        manage({ action: "create-plan", ...input, editors }),
        /Allowances/,
      );
    await manage({ action: "create-plan", ...input });
    const plan = (await getPlans(root.headers)).find(
      (p) => p.name === input.name,
    )!;
    assert.equal(plan.allowances.storageBytes, 500_000_000);
    assert.equal(plan.allowances.transferBytes, 2_000_000_000);
    assert.equal((await getTenantPlan(admin.headers, one)).subscription, null);
    const assignments = await Promise.allSettled([
      manage({ action: "assign", planID: plan.id, revision: "0" }),
      manage({ action: "assign", planID: plan.id, revision: "0" }),
    ]);
    assert.equal(assignments.filter((r) => r.status === "fulfilled").length, 1);
    await manage({ action: "extras", revision: "1", storageMB: "1500" });
    assert.equal(
      (await getTenantPlan(admin.headers, one)).subscription!.effective
        .storageBytes,
      2_000_000_000,
    );
    await manage({ action: "create-plan", name: input.name + " unlimited" });
    const unlimited = (await getPlans(root.headers)).find(
      (p) => p.name === input.name + " unlimited",
    )!;
    assert.equal(
      (await getTenantPlan(admin.headers, one)).subscription!.base.storageBytes,
      500_000_000,
    );
    await manage({ action: "assign", planID: unlimited.id, revision: "2" });
    const updated = (await getTenantPlan(admin.headers, one)).subscription!;
    assert.equal(updated.extras.storageBytes, 1_500_000_000);
    assert.equal(updated.effective.storageBytes, null);
    const issue = async (revision: string) => {
      const result = await manage({
        action: "create-offer",
        planID: plan.id,
        ...input,
        storageMB: "750",
        terms: "Manual invoice: CHF 10 per month",
        revision,
        expiresDays: "14",
      });
      assert.equal(new URL(result.offerURL!).origin, origin);
      return result.offerURL!.split("/").at(-1)!;
    };
    const stale = await issue("3");
    await manage({ action: "extras", revision: "3", storageMB: "2000" });
    await assert.rejects(acceptOffer(admin.headers, stale), /stale/);
    assert.equal((await getOffer(admin.headers, stale)).status, "stale");
    assert.equal(
      (await getOffer(admin.headers, stale)).extras.storageBytes,
      1_500_000_000,
    );
    const token = await issue("4");
    const stored = (
      await database.query(
        "SELECT * FROM webdock_auth.plan_offer WHERE token_hash=$1",
        [createHash("sha256").update(token).digest("hex")],
      )
    ).rows[0];
    assert.ok(!JSON.stringify(stored).includes(token));
    const preview = await getOffer(admin.headers, token);
    assert.equal(preview.canAccept, true);
    assert.equal(preview.effective.storageBytes, 2_750_000_000);
    assert.equal(preview.terms, "Manual invoice: CHF 10 per month");
    assert.equal((await getOffer(member.headers, token)).canAccept, false);
    await assert.rejects(getOffer(other.headers, token));
    await assert.rejects(acceptOffer(other.headers, token));
    await assert.rejects(acceptOffer(member.headers, token), /administrator/);
    await assert.rejects(acceptOffer(root.headers, token), /administrator/);
    const accepts = await Promise.all([
      acceptOffer(admin.headers, token),
      acceptOffer(admin.headers, token),
    ]);
    assert.ok(accepts.some((a) => a.message.includes("already accepted")));
    const accepted = (await getTenantPlan(admin.headers, one)).subscription!;
    assert.equal(accepted.revision, 5);
    assert.equal(accepted.base.storageBytes, 750_000_000);
    assert.equal(accepted.extras.storageBytes, 2_000_000_000);
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS count FROM webdock_auth.access_event WHERE target_id=$1 AND action='plans-accept-offer'",
          [one],
        )
      ).rows[0].count,
      1,
    );
    await assert.rejects(acceptOffer(other.headers, token));
    const revoked = await issue("5");
    const offers = (await getTenantPlan(root.headers, one)).offers;
    await manage({
      action: "revoke-offer",
      offerID: offers.find((o) => o.status === "pending")!.id,
    });
    await assert.rejects(acceptOffer(admin.headers, revoked), /revoked/);
    const expired = await issue("5");
    await database.query(
      "UPDATE webdock_auth.plan_offer SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
      [createHash("sha256").update(expired).digest("hex")],
    );
    await assert.rejects(acceptOffer(admin.headers, expired), /expired/);
    const current = await issue("5");
    await manage({ action: "extras", revision: "5", storageMB: "3000" });
    assert.equal(
      (await getOffer(admin.headers, token)).extras.storageBytes,
      2_000_000_000,
    );
    assert.equal(
      (await getOffer(admin.headers, token)).effective.storageBytes,
      2_750_000_000,
    );
    const history = (
      await database.query(
        "SELECT * FROM webdock_auth.subscription_history WHERE customer_id=$1 ORDER BY revision",
        [one],
      )
    ).rows;
    assert.deepEqual(
      history.map((row) => row.revision),
      [1, 2, 3, 4, 5, 6],
    );
    assert.equal(history[1].extras.storageBytes, 1_500_000_000);
    assert.equal(history[4].extras.storageBytes, 2_000_000_000);
    assert.equal(history[4].actor_id, admin.id);
    assert.equal(history[5].actor_id, root.id);
    // Customer-specific plans have no reusable template, even when replacing one.
    const catalog = await getPlans(root.headers);
    const custom = {
      action: "assign-custom",
      customerID: two,
      revision: "0",
      ...input,
      name: "Customer-only plan",
      "hosting.apps": "2",
      "hosting.cpuMillicores": "0.5 cores",
    };
    await assert.rejects(managePlans(other.headers, custom), /operator/);
    await assert.rejects(managePlans(member.headers, custom), /operator/);
    await assert.rejects(managePlans(new Headers(), custom));
    await assert.rejects(
      manage({ ...custom, name: " " }),
      /valid offer or plan/,
    );
    await assert.rejects(manage({ ...custom, storageMB: "-1" }), /Allowances/);
    await assert.rejects(
      manage({ ...custom, "hosting.apps": "-1" }),
      /hosting allowances/,
    );
    await manage(custom);
    const standalone = (await getTenantPlan(other.headers, two)).subscription!;
    assert.equal(standalone.name, custom.name);
    assert.equal(standalone.base.storageBytes, 500_000_000);
    assert.equal(standalone.base.hosting?.apps, 2);
    assert.equal(standalone.base.hosting?.cpuMillicores, 500);
    assert.equal(standalone.extras.storageBytes, 0);
    assert.equal(
      (
        await database.query(
          "SELECT plan_id FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
          [two],
        )
      ).rows[0].plan_id,
      null,
    );
    await assert.rejects(
      manage({ action: "assign", customerID: one, planID: two, revision: "6" }),
      /Plan not found/,
    );
    await assert.rejects(getTenantPlan(admin.headers, two));
    await manage({
      action: "extras",
      customerID: two,
      revision: "1",
      storageMB: "250",
      "hosting.apps": "1",
    });
    const replacements = await Promise.allSettled([
      manage({ ...custom, revision: "2", storageMB: "600" }),
      manage({ ...custom, revision: "2", storageMB: "700" }),
    ]);
    assert.equal(
      replacements.filter((result) => result.status === "fulfilled").length,
      1,
    );
    const replaced = (await getTenantPlan(other.headers, two)).subscription!;
    assert.equal(replaced.revision, 3);
    assert.equal(
      replaced.effective.storageBytes,
      replaced.base.storageBytes! + 250_000_000,
    );
    assert.equal(replaced.effective.hosting?.apps, 3);
    await manage({ ...custom, customerID: one, revision: "6" });
    assert.equal(
      (await getTenantPlan(admin.headers, one)).subscription!.extras
        .storageBytes,
      3_000_000_000,
    );
    const customHistory = (
      await database.query(
        "SELECT * FROM webdock_auth.subscription_history WHERE customer_id=$1 AND revision=7",
        [one],
      )
    ).rows[0];
    assert.equal(customHistory.plan_id, null);
    assert.equal(customHistory.actor_id, root.id);
    assert.equal(customHistory.name, custom.name);
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS count FROM webdock_auth.access_event WHERE target_id=$1 AND action='plans-assign-custom'",
          [one],
        )
      ).rows[0].count,
      1,
    );
    const customOffer = await manage({
      action: "create-offer",
      ...input,
      name: "Private agreed package",
      revision: "7",
    });
    const customToken = customOffer.offerURL!.split("/").at(-1)!;
    await manage({
      ...custom,
      customerID: one,
      revision: "7",
      "hosting.apps": "3",
    });
    await assert.rejects(acceptOffer(admin.headers, customToken), /stale/);
    assert.deepEqual(await getPlans(root.headers), catalog);
    assert.equal(
      (
        await database.query(
          "SELECT base FROM webdock_auth.subscription_history WHERE customer_id=$1 AND revision=7",
          [one],
        )
      ).rows[0].base.hosting.apps,
      2,
    );
    await database.query(
      "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
      [admin.id],
    );
    await assert.rejects(acceptOffer(admin.headers, current), /administrator/);
    await assert.rejects(acceptOffer(admin.headers, token), /administrator/);
    await database.query(
      "UPDATE webdock_auth.member SET role='admin' WHERE \"userId\"=$1",
      [admin.id],
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
      [admin.id],
    );
    await assert.rejects(acceptOffer(admin.headers, current));
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [admin.id],
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [one],
    );
    await assert.rejects(acceptOffer(admin.headers, current));
    await assert.rejects(getTenantPlan(admin.headers, one));
    assert.equal(
      (await getTenantPlan(root.headers, one)).subscription!.revision,
      8,
    );
    await assert.rejects(
      manage({ ...custom, customerID: one, revision: "8" }),
      /Active tenant/,
    );
    await assert.rejects(getOffer(root.headers, current));
    await assert.rejects(
      manage({ action: "assign", planID: plan.id, revision: "5" }),
    );
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
      [root.id],
    );
    await assert.rejects(
      managePlans(root.headers, { action: "create-plan", ...input }),
    );
    await assert.rejects(
      managePlans(root.headers, { ...custom, revision: "3" }),
      /security setup/,
    );
  } finally {
    // Fixture identities and IDs are unique to this test; never clean by a shared display name.
    await database.query(
      "DELETE FROM webdock_auth.plan_offer WHERE customer_id=ANY($1)",
      [customerIDs],
    );
    await database.query(
      "DELETE FROM webdock_auth.subscription_history WHERE customer_id=ANY($1)",
      [customerIDs],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=ANY($1)",
      [customerIDs],
    );
    await database.query(
      "DELETE FROM webdock_auth.plan_template WHERE created_by=$1",
      [root.id],
    );
    await database.query(
      "DELETE FROM webdock_auth.access_event WHERE actor_id=ANY($1)",
      [[root.id, admin.id, other.id, member.id]],
    );
    await database.query(
      'DELETE FROM webdock_auth.member WHERE "organizationId"=ANY($1)',
      [organizationIDs],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [customerIDs],
    );
    await database.query(
      "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
      [customerIDs],
    );
    await database.query(
      "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
      [organizationIDs],
    );
    await database.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
      [root.id, admin.id, other.id, member.id],
    ]);
  }
});
