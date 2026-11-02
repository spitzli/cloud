import { expect, test } from "bun:test";
import { buildDatabaseRls } from "../../../legacy/control-plane/src/db/database-rls-schema";

test("native authority guards coexist with signed external proof branches", () => {
  const plan=buildDatabaseRls([
    {schema:"webdock_auth",name:"clerk_identity",columns:["issuer","clerk_user_id","user_id","enabled","legacy_eligible"]},
    {schema:"webdock_auth",name:"access_invitation_state",columns:["invitation_id","organization_id","user_id","generation"]},
    {schema:"webdock_auth",name:"access_delivery_job",columns:["id","organization_id","actor_id"]},
    {schema:"webdock_auth",name:"local_identity_activation",columns:["user_id","bindings"]},
    {schema:"webdock_auth",name:"local_organization_activation",columns:["organization_id","bindings"]},
  ]);
  expect(plan.unknownTables).toEqual([]);
  expect(plan.sql).toContain("IF NOT webdock_rls.native_identity_authority(actor_row.id) THEN RETURN NULL");
  expect(plan.sql).toContain("IF NOT webdock_rls.native_organization_authority(org) THEN RETURN NULL");
  expect(plan.sql).toContain("customer_id=customer AND NOT webdock_rls.native_organization_authority(organization_id)");
  const actor=plan.sql.slice(plan.sql.indexOf("CREATE OR REPLACE FUNCTION webdock_rls.actor()"),plan.sql.indexOf("CREATE OR REPLACE FUNCTION webdock_rls.operator()"));
  expect(actor.indexOf("IF s ? 'clerk'")).toBeLessThan(actor.indexOf("native_identity_authority(actor_row.id)"));
  expect(actor.indexOf("RETURN jsonb_build_object")).toBeLessThan(actor.indexOf("native_identity_authority(actor_row.id)"));
  expect(plan.sql).not.toContain('GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock_auth"."local_identity_activation" TO webdock_auth_bootstrap');
  expect(plan.sql).toContain('GRANT SELECT("user_id",bindings) ON "webdock_auth"."local_identity_activation" TO webdock_auth_bootstrap');
});
