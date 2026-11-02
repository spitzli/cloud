import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import pg from "pg";
import { assertTestDatabase } from "../helpers/database";
import { createSetupFixture } from "../helpers/setup-fixture";
import { database, closeDatabases } from "../../src/db/runtime";
import { executeHosting } from "../../src/modules/hosting/service";
import { HostingError, type ProjectOverview } from "@webdock/hosting-contracts";
afterEach(restoreMocks);

assertTestDatabase(process.env.DATABASE_URL!);
test.after(closeDatabases);

test("project overview uses one authorized transaction, preserves display bounds and live binding health", async (t) => {
  const fixture = await createSetupFixture(database);
  t.after(async () => {
    await database.query(
      "DELETE FROM webdock_auth.git_source WHERE project_id=$1",
      [fixture.projectID],
    );
    await database.query(
      "DELETE FROM webdock_auth.git_repository WHERE customer_id=$1",
      [fixture.customerID],
    );
    await database.query(
      "DELETE FROM webdock_auth.git_connection WHERE customer_id=$1",
      [fixture.customerID],
    );
    await fixture.cleanup();
  });
  const call = () =>
    executeHosting(fixture.actor, {
      action: "projects.overview",
      projectID: fixture.projectID,
    }) as Promise<ProjectOverview>;
  const statements: string[] = [];
  const query = pg.Client.prototype.query;
  const tracked = mockMethod(
    pg.Client.prototype,
    "query",
    function (this: pg.Client, ...args: any[]) {
      statements.push(typeof args[0] === "string" ? args[0] : args[0].text);
      return Reflect.apply(query, this, args);
    },
  );
  const empty = await call();
  tracked.mock.restore();
  assert.equal(statements.filter((sql) => sql === "BEGIN").length, 1);
  assert.equal(
    statements.filter((sql) => sql.includes("FOR SHARE OF u,s")).length,
    1,
  );
  const presentationStatements = statements.filter(
    (sql) =>
      !sql.includes("webdock_auth.clerk_identity") &&
      !sql.includes("webdock_auth.clerk_organization"),
  );
  assert.ok(
    // Identity ports add one preview and two tenant-mapping reads.
    presentationStatements.length <= 18,
    `empty overview query budget exceeded: ${presentationStatements.length}`,
  );
  assert.deepEqual(empty, {
    project: {
      name: "Setup fixture",
      configured: true,
      provider: "k3s",
      canManage: true,
    },
    applications: [],
    databaseCount: 0,
    bindings: [],
    setups: [],
    sources: [],
  });
  const apps = (
    await database.query(
      `INSERT INTO webdock_auth.hosting_app(project_id,name,spec,status,lifecycle)
    SELECT $1,'App '||lpad(i::text,3,'0'),'{}','pending','draft' FROM generate_series(1,205) i RETURNING id,name`,
      [fixture.projectID],
    )
  ).rows;
  await database.query(
    `INSERT INTO webdock_auth.hosting_app(project_id,name,spec,status) VALUES($1,'Deleted app','{}','deleted')`,
    [fixture.projectID],
  );
  await database.query(
    `INSERT INTO webdock_auth.hosting_setup(customer_id,project_id,subject,configuration,environment_encrypted,state,updated_at)
    SELECT $1,$2,$3,jsonb_build_object('name','Setup '||i,'privateValue','secret'),'not-needed-for-summary',CASE WHEN i=5 THEN 'ready' ELSE 'draft' END,now()+i*interval '1 second' FROM generate_series(0,5) i`,
    [fixture.customerID, fixture.projectID, fixture.actor.subject],
  );
  await database.query(
    `INSERT INTO webdock_auth.hosting_service_binding(customer_id,project_id,app_id,service,variable,state)
    VALUES($1,$2,$3,'{"kind":"mail","variable":"MAIL","connectionID":"1","domainID":"2"}','MAIL','connected'),
    ($1,$2,$3,'{"kind":"mail","variable":"OLD","connectionID":"1","domainID":"2"}','OLD','disconnected')`,
    [fixture.customerID, fixture.projectID, apps[0].id],
  );
  const connection = (
    await database.query(
      "INSERT INTO webdock_auth.git_connection(customer_id,installation_id,account_id,account_login,permissions) VALUES($1,$2,'fixture','fixture','{}') RETURNING id",
      [fixture.customerID, fixture.customerID],
    )
  ).rows[0];
  await database.query(
    `INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'repo','{"fullName":"fixture/repository"}')`,
    [connection.id, fixture.customerID],
  );
  await database.query(
    "INSERT INTO webdock_auth.git_source(customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id,enabled,build_environment_encrypted) VALUES($1,$2,$3,'repo','main','.','dockerfile',$4,false,'not-needed-for-summary')",
    [fixture.customerID, fixture.projectID, connection.id, apps[0].id],
  );
  const result = await call();
  assert.deepEqual(
    result.sources,
    [
      {
        targetID: apps[0].id,
        repositoryName: "fixture/repository",
        branch: "main",
      },
    ],
    "disabled sources remain visible exactly as before",
  );
  assert.equal(result.applications.length, 200);
  assert.equal(result.applications[0].name, "App 001");
  assert.equal(result.applications.at(-1)?.name, "App 200");
  assert.deepEqual(
    result.setups.map((s) => s.name),
    ["Setup 4", "Setup 3", "Setup 2", "Setup 1"],
    "filter ready after selecting latest five",
  );
  assert.equal(result.bindings.length, 1);
  assert.equal(
    result.bindings[0].state,
    "blocked",
    "revoked/unready Mail still overrides stored connected status",
  );
  assert.equal(JSON.stringify(result).includes("privateValue"), false);
  assert.equal(
    JSON.stringify(result).includes("not-needed-for-summary"),
    false,
  );
  assert.deepEqual(Object.keys(result.bindings[0]).sort(), [
    "appID",
    "id",
    "service",
    "state",
  ]);
});

test("project overview retains live session, MFA, tenant, project and preview restrictions", async (t) => {
  const fixture = await createSetupFixture(database),
    other = await createSetupFixture(database);
  t.after(fixture.cleanup);
  t.after(other.cleanup);
  const call = (actor = fixture.actor, projectID = fixture.projectID) =>
    executeHosting(actor, {
      action: "projects.overview",
      projectID,
    }) as Promise<ProjectOverview>;
  const denied = async (run: () => Promise<unknown>) =>
    assert.rejects(
      run,
      (error) =>
        error instanceof HostingError && [401, 403, 404].includes(error.status),
    );
  await denied(() => call(other.actor));
  await denied(() => call({ ...fixture.actor, subject: other.actor.subject }));
  await denied(() => call(fixture.actor, other.projectID));
  await denied(() => call({ ...fixture.actor, scopes: [] }));
  for (const [field, value] of [
    ["banned", true],
    ["emailVerified", false],
    ["mustChangePassword", true],
  ] as const) {
    await database.query(
      `UPDATE webdock_auth."user" SET "${field}"=$2 WHERE id=$1`,
      [fixture.actor.subject, value],
    );
    await denied(() => call());
    await database.query(
      `UPDATE webdock_auth."user" SET "${field}"=$2 WHERE id=$1`,
      [fixture.actor.subject, !value],
    );
  }
  await database.query(
    'UPDATE webdock_auth."user" SET role=\'operator\',"twoFactorEnabled"=false WHERE id=$1',
    [fixture.actor.subject],
  );
  await denied(() => call());
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [fixture.actor.subject],
  );
  assert.equal((await call()).project.canManage, true);
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [fixture.customerID],
    )
  ).rows[0].organization_id;
  await database.query(
    `INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '1 hour')`,
    [fixture.actor.sessionID, fixture.actor.subject, fixture.customerID, org],
  );
  t.after(() =>
    database.query(
      "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
      [fixture.actor.sessionID],
    ),
  );
  assert.equal((await call()).project.canManage, false);
  await denied(() => call(fixture.actor, other.projectID));
  await database.query(
    "UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id=$1",
    [fixture.actor.sessionID],
  );
  await denied(() => call());
  await database.query(
    "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
    [fixture.actor.sessionID],
  );
  await database.query(
    'UPDATE webdock_auth."user" SET role=\'user\',"twoFactorEnabled"=false WHERE id=$1',
    [fixture.actor.subject],
  );
  for (const table of ["webdock_admin.customers", "webdock_admin.projects"]) {
    const id = table.endsWith("customers")
      ? fixture.customerID
      : fixture.projectID;
    await database.query(`UPDATE ${table} SET status='archived' WHERE id=$1`, [
      id,
    ]);
    await denied(() => call());
    await database.query(`UPDATE ${table} SET status='active' WHERE id=$1`, [
      id,
    ]);
  }
  await database.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
    fixture.actor.subject,
  ]);
  await denied(() => call());
  await database.query(
    'UPDATE webdock_auth."user" SET role=\'operator\',"twoFactorEnabled"=true WHERE id=$1',
    [fixture.actor.subject],
  );
  assert.equal((await call()).project.canManage, true);
  await database.query("DELETE FROM webdock_auth.session WHERE id=$1", [
    fixture.actor.sessionID,
  ]);
  await denied(() => call());
});
