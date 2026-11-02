import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { platformSchemaSQL } from "../../src/modules/platform/platform";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import {
  getTenantMailService,
  manageTenantMailService,
} from "../../src/modules/mail/mail-service";
import {
  nativeMailSchemaSQL,
  nativeMailClusterSchemaSQL,
} from "@webdock/mail-core/schema";
import { mailBudgetSchemaSQL } from "../../src/modules/mail/mail-budget";
import {
  mailInstanceDemand,
  nativeClusterDemand,
} from "@webdock/mail-core/capacity";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import {
  getMailService,
  requestMailService,
  claimMailOperation,
  finishMailOperation,
} from "@webdock/mail-core";

const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local test database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});

async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `native-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Native mail test",
    password,
    operator,
    mustChangePassword: false,
  });
  const response = await auth.api.signInEmail({
    body: { email: user.email, password },
    asResponse: true,
  });
  assert.equal(response.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return {
    ...user,
    headers: new Headers({
      Cookie: response.headers
        .getSetCookie()
        .map((value) => value.split(";")[0])
        .join("; "),
    }),
  };
}

test("activation is explicit and idempotent, pending changes serialize, and suspension retains the instance", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(platformSchemaSQL);
  await database.query(nativeMailSchemaSQL);
  await database.query(nativeMailSchemaSQL);
  const previous = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const actor = (
    await database.query(
      'INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword","twoFactorEnabled") VALUES(\'Mail operator\',\'native-mail-test@example.invalid\',true,\'operator\',false,true) RETURNING id',
    )
  ).rows[0].id;
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Native Mail A') RETURNING id",
    )
  ).rows[0].id;
  const request = (enabled: boolean, expectedRevision: string) =>
    requestMailService(database, {
      customerID: customer,
      enabled,
      expectedRevision,
      actorID: actor,
      hostID: "local-test",
    });
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings",
    );
    assert.deepEqual(await getMailService(database, customer), {
      customerID: customer,
      enabled: false,
      state: "disabled",
      revision: "0",
    });
    await request(false, "0");
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].n,
      0,
    );
    await database.query(mailBudgetSchemaSQL);
    const reference = (
      await database.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await database.query(
      "INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved) VALUES($1,'2026-10-01',1)",
      [customer],
    );
    await database.query(
      `INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,period_start,units,send_path) VALUES($1,$2,'2026-10-01',1,'{"kind":"direct"}')`,
      [reference, customer],
    );
    await assert.rejects(request(true, "0"), /Reconcile pending sends/);
    assert.equal(
      (
        await database.query(
          "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rowCount,
      0,
    );
    await database.query(
      "UPDATE webdock_mail.budget_reservation SET state='accepted' WHERE reference_id=$1",
      [reference],
    );
    await database.query(
      "UPDATE webdock_mail.month_usage SET reserved=0,accepted=1 WHERE customer_id=$1",
      [customer],
    );
    const results = await Promise.all(
      Array.from({ length: 6 }, () => request(true, "0")),
    );
    assert.ok(
      results.every(
        (result) => result.revision === "1" && result.state === "pending",
      ),
    );
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_mail.operation WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].n,
      1,
    );
    const claims = (
      await Promise.all([
        claimMailOperation(database, "local-test"),
        claimMailOperation(database, "local-test"),
      ])
    ).filter((value) => value !== null);
    assert.equal(claims.length, 1);
    const first = claims[0]!;
    assert.equal(first.customerID, customer);
    assert.equal(first.instanceKey, `mail-${customer}`);
    assert.equal(
      (await getMailService(database, customer)).state,
      "provisioning",
    );
    await assert.rejects(request(false, "0"), /changed/);
    await request(false, "1");
    assert.equal(await claimMailOperation(database, "local-test"), null);
    assert.equal(
      await finishMailOperation(
        database,
        { ...first, leaseToken: "wrong" },
        "succeeded",
      ),
      false,
    );
    assert.equal(await finishMailOperation(database, first, "succeeded"), true);
    assert.deepEqual(await getMailService(database, customer), {
      customerID: customer,
      enabled: false,
      state: "pending",
      revision: "2",
    });
    const stop = await claimMailOperation(database, "local-test");
    assert.ok(stop && !stop.enabled);
    assert.equal(stop.instanceKey, first.instanceKey);
    assert.equal(await finishMailOperation(database, stop, "succeeded"), true);
    assert.equal((await getMailService(database, customer)).state, "suspended");
    await request(true, "2");
    const restart = await claimMailOperation(database, "local-test");
    assert.ok(restart);
    assert.equal(restart.instanceKey, first.instanceKey);
    await database.query(
      "UPDATE webdock_mail.operation SET lease_until=now()-interval '1 second' WHERE id=$1",
      [restart.id],
    );
    assert.equal(
      await finishMailOperation(database, restart, "succeeded"),
      false,
    );
    assert.equal(await claimMailOperation(database, "local-test"), null);
    assert.equal(
      (await getMailService(database, customer)).state,
      "needs_review",
    );
    await request(true, "3");
    assert.equal(await claimMailOperation(database, "local-test"), null);
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].n,
      1,
    );
    const retry = {
      customerID: customer,
      enabled: true,
      expectedRevision: "3",
      actorID: actor,
      hostID: "local-test",
      reconcile: true,
    };
    await assert.rejects(requestMailService(database, retry), /verified/);
    await database.query(
      "INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at) VALUES($1,'fixture',now())",
      [customer],
    );
    await assert.rejects(
      requestMailService(database, { ...retry, expectedRevision: "2" }),
      /changed/,
    );
    const repaired = await requestMailService(database, retry);
    assert.equal(repaired.state, "pending");
    assert.equal(repaired.revision, "4");
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.operation WHERE id=$1",
          [restart.id],
        )
      ).rows[0].state,
      "superseded",
    );
    const recheck = await claimMailOperation(database, "local-test");
    assert.ok(recheck);
    await finishMailOperation(database, recheck, "succeeded");
    assert.equal((await getMailService(database, customer)).state, "ready");
  } finally {
    await database.query(
      "DELETE FROM webdock_mail.budget_reservation WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_mail.month_usage WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_mail.instance WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_mail.operation WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_mail.service WHERE customer_id=$1",
      [customer],
    );
    await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
      actor,
    ]);
    const mapping = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",
        [customer],
      )
    ).rows[0];
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      mapping.organization_id,
    ]);
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [previous.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});

test("Mail activation requires current operator access and customer reads remain tenant-scoped", async () => {
  await database.query(nativeMailSchemaSQL);
  const oldFlag = process.env.WEBDOCK_NATIVE_MAIL_ENABLED;
  process.env.WEBDOCK_NATIVE_MAIL_ENABLED = "true";
  const root = await identity(true),
    member = await identity(),
    outsider = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Native Mail Permissions') RETURNING id",
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
    [org, member.id],
  );
  try {
    await assert.rejects(getTenantMailService(outsider.headers, customer));
    const view = await getTenantMailService(member.headers, customer);
    assert.equal(view!.service.enabled, false);
    assert.equal(view!.canActivate, false);
    await assert.rejects(
      manageTenantMailService(member.headers, customer, {
        action: "activate",
        revision: "0",
      }),
    );
    assert.equal(
      (await getTenantMailService(root.headers, customer))!.service.state,
      "disabled",
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
      [root.id],
    );
    await assert.rejects(
      manageTenantMailService(root.headers, customer, {
        action: "activate",
        revision: "0",
      }),
    );
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].n,
      0,
    );
  } finally {
    if (oldFlag === undefined) delete process.env.WEBDOCK_NATIVE_MAIL_ENABLED;
    else process.env.WEBDOCK_NATIVE_MAIL_ENABLED = oldFlag;
    await database.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
      [root.id, member.id, outsider.id],
    ]);
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      org,
    ]);
  }
});

test("managed Mail reserves cluster capacity atomically and retained volumes remain charged after suspension", async () => {
  await database.query(hostingSchemaSQL);
  await database.query(nativeMailClusterSchemaSQL);
  const root = await identity(true);
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail capacity A'),(webdock_auth.next_snowflake(),'Mail capacity B') RETURNING id",
    )
  ).rows.map((row) => row.id as string);
  const cluster = (
    await database.query(
      `INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence,verified,capacity)
    VALUES('Mail test cluster','Contabo','DE','Germany','Local fixture',true,$1) RETURNING id`,
      [JSON.stringify(mailInstanceDemand)],
    )
  ).rows[0].id;
  await database.query(
    `INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation)
    VALUES($1,$2,1,now(),$3)`,
    [
      cluster,
      randomBytes(32).toString("hex"),
      JSON.stringify({
        version: "v1.36.5+k3s1",
        capabilities: { nativeMail: { version: 1 }, storageVersion: 1 },
      }),
    ],
  );
  const previous = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings",
    );
    const result = await Promise.allSettled(
      customers.map((id) =>
        requestMailService(database, {
          customerID: id,
          enabled: true,
          expectedRevision: "0",
          actorID: root.id,
          hostID: cluster,
        }),
      ),
    );
    assert.equal(
      result.filter((item) => item.status === "fulfilled").length,
      1,
    );
    const rejection = result.find(
      (item) => item.status === "rejected",
    ) as PromiseRejectedResult;
    assert.match(rejection.reason.message, /capacity/);
    assert.equal(
      (await nativeClusterDemand(database, cluster)).memoryBytes,
      1073741824,
    );
    const job = await claimMailOperation(database, cluster);
    assert.ok(job);
    await finishMailOperation(database, job, "succeeded");
    await requestMailService(database, {
      customerID: job.customerID,
      enabled: false,
      expectedRevision: "1",
      actorID: root.id,
      hostID: cluster,
    });
    const stop = await claimMailOperation(database, cluster);
    assert.ok(stop);
    await finishMailOperation(database, stop, "succeeded");
    const retained = await nativeClusterDemand(database, cluster);
    assert.equal(retained.cpuMillicores, 0);
    assert.equal(retained.memoryBytes, 0);
    assert.equal(retained.volumeBytes, 10737418240);
    const other = customers.find((id) => id !== job.customerID)!;
    await assert.rejects(
      requestMailService(database, {
        customerID: other,
        enabled: true,
        expectedRevision: "0",
        actorID: root.id,
        hostID: cluster,
      }),
      /capacity/,
    );
  } finally {
    await database.query(
      "DELETE FROM webdock_mail.cluster_reservation WHERE customer_id=ANY($1)",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_mail.operation WHERE customer_id=ANY($1)",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_mail.service WHERE customer_id=ANY($1)",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
      [cluster],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
      [cluster],
    );
    const mappings = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
        [customers],
      )
    ).rows;
    await database.query(
      "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
      [mappings.map((row) => row.organization_id)],
    );
    await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
      root.id,
    ]);
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [previous.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});

test('draining rejects first incoming activation but permits resuming a retained suspended service', async () => {
  const oldModules = process.env.WEBDOCK_MODULES, oldFlag = process.env.WEBDOCK_NATIVE_MAIL_ENABLED;
  const settings = (await database.query('SELECT settings FROM webdock_auth.platform_settings WHERE id=true')).rows[0];
  const root = await identity(true);
  const customer = (await database.query("INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Draining fixture') RETURNING id")).rows[0].id;
  try {
    process.env.WEBDOCK_NATIVE_MAIL_ENABLED = 'true';
    process.env.WEBDOCK_MODULES = JSON.stringify({configVersion:1,modules:{mail:{state:'draining'}}});
    await database.query(`INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings`);
    await assert.rejects(manageTenantMailService(root.headers,customer,{action:'activate',revision:'0'}),/draining/);
    await database.query("INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,state) VALUES($1,$2,'primary','suspended')",[customer,'mail-'+customer]);
    await manageTenantMailService(root.headers,customer,{action:'activate',revision:'1'});
    assert.equal((await getMailService(database,customer)).enabled,true);
  } finally {
    if(oldModules===undefined)delete process.env.WEBDOCK_MODULES;else process.env.WEBDOCK_MODULES=oldModules;
    if(oldFlag===undefined)delete process.env.WEBDOCK_NATIVE_MAIL_ENABLED;else process.env.WEBDOCK_NATIVE_MAIL_ENABLED=oldFlag;
    await database.query('DELETE FROM webdock_mail.operation WHERE customer_id=$1',[customer]);
    await database.query('DELETE FROM webdock_mail.service WHERE customer_id=$1',[customer]);
    if(settings)await database.query('UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true',[JSON.stringify(settings.settings)]);
    else await database.query('DELETE FROM webdock_auth.platform_settings WHERE id=true');
  }
});
