import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { authorizeMailSession, assertMailAccess } =
  await import("../../src/modules/mail/product/access");
const { createMailKey, authorizeMailKey, revokeMailKey } =
  await import("../../src/modules/mail/product/api-keys");
test.after(() => database.end());
test("current session, membership, tenant, connection and preview boundaries", async () => {
  await withMailProductFixture(async (f) => {
    await authorizeMailSession(
      f.adminHeaders,
      f.actorA.customerID,
      f.actorA.connectionID,
      "manage",
    );
    const member = await authorizeMailSession(
      f.memberHeaders,
      f.actorA.customerID,
      f.actorA.connectionID,
      "send",
    );
    await assert.rejects(assertMailAccess(member, "manage"));
    await assert.rejects(
      authorizeMailSession(
        f.adminHeaders,
        f.actorB.customerID,
        f.actorB.connectionID,
        "read",
      ),
    );
    await f.db.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
      f.member.id,
    ]);
    await assert.rejects(assertMailAccess(member, "read"));
    await f.db.query('UPDATE webdock_auth."user" SET banned=true WHERE id=$1', [
      f.admin.id,
    ]);
    await assert.rejects(assertMailAccess(f.actorA, "read"));
    await f.db.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [f.admin.id],
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET state='paused' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await assertMailAccess(f.actorA, "read");
    await assert.rejects(assertMailAccess(f.actorA, "send"));
    await f.db.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [f.actorA.customerID],
    );
    await assert.rejects(assertMailAccess(f.actorA, "read"));
  });
});
test("keys are hashed, scoped, instantly revocable and survive creator departure", async () => {
  await withMailProductFixture(async (f) => {
    const key = await createMailKey(f.actorA, {
      name: "orders",
      permission: "sending_access",
      domainID: f.domainA.id,
    });
    assert.match(key.key, /^wdm_[A-Za-z0-9_-]{43}$/);
    const stored = (
      await f.db.query("SELECT * FROM webdock_auth.mail_api_key WHERE id=$1", [
        key.id,
      ])
    ).rows[0];
    assert.ok(!JSON.stringify(stored).includes(key.key));
    const req = new Request("https://auth.webdock.dev/api/mail/v1/emails", {
      headers: { Authorization: "Bearer " + key.key },
    });
    const actor = await authorizeMailKey(req, "sending_access");
    await assert.rejects(assertMailAccess(actor, "read"));
    await assert.rejects(authorizeMailKey(req, "full_access"));
    await f.db.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
      f.admin.id,
    ]);
    await assertMailAccess(actor, "send");
    const operator = await authorizeMailSession(
      f.operatorHeaders,
      f.actorA.customerID,
      f.actorA.connectionID,
      "manage",
    );
    await revokeMailKey(operator, key.id);
    await assert.rejects(authorizeMailKey(req, "sending_access"));
  });
});

test("preview and incomplete operators cannot obtain authority; full keys cannot widen scope", async () => {
  await withMailProductFixture(async (f) => {
    const { startTenantPreview, exitTenantPreview } =
      await import("../../src/modules/access/tenant-preview");
    const who = {
      userID: f.operator.id,
      sessionID: f.operator.principal.sessionID,
    };
    await startTenantPreview(who, f.actorA.customerID);
    await assert.rejects(
      authorizeMailSession(
        f.operatorHeaders,
        f.actorA.customerID,
        f.actorA.connectionID,
        "read",
      ),
    );
    await f.db.query(
      "UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 minute' WHERE session_id=$1",
      [who.sessionID],
    );
    await assert.rejects(
      authorizeMailSession(
        f.operatorHeaders,
        f.actorA.customerID,
        f.actorA.connectionID,
        "manage",
      ),
    );
    await exitTenantPreview(who);
    await f.db.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
      [f.operator.id],
    );
    await assert.rejects(
      authorizeMailSession(
        f.operatorHeaders,
        f.actorA.customerID,
        f.actorA.connectionID,
        "manage",
      ),
    );
    const key = await createMailKey(f.actorA, {
      name: "limited full",
      permission: "full_access",
      domainID: f.domainA.id,
    });
    const actor = await authorizeMailKey(
      new Request("https://auth.webdock.dev/api/mail/v1/api-keys", {
        headers: { Authorization: "Bearer " + key.key },
      }),
      "full_access",
    );
    await assert.rejects(
      createMailKey(actor, { name: "wider", permission: "full_access" }),
    );
    await assert.rejects(assertMailAccess(actor, "operate"));
    await createMailKey(actor, {
      name: "same",
      permission: "sending_access",
      domainID: f.domainA.id,
    });
  });
});

test('previously authorized actors lose access after session expiry', async () => {
  await withMailProductFixture(async (f) => {
    const actor=await authorizeMailSession(f.adminHeaders,f.actorA.customerID,f.actorA.connectionID,'manage');
    assert.equal(actor.principal.kind,'session');
    if(actor.principal.kind!=='session')throw Error('Session fixture required');
    const {resolveSessionReference}=await import('../../src/modules/auth/principal');
    assert.deepEqual(await resolveSessionReference(f.adminHeaders),{subject:actor.principal.userID,sessionID:actor.principal.sessionID});
    await f.db.query('UPDATE webdock_auth.session SET "expiresAt"=now()-interval \'1 second\' WHERE id=$1',[actor.principal.sessionID]);
    await assert.rejects(assertMailAccess(actor,'read'));
    assert.equal(await resolveSessionReference(f.adminHeaders),null);
  });
});
