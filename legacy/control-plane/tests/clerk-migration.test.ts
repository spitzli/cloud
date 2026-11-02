import { test, expect } from "bun:test";
import { migrationManifest } from "../scripts/migrate-clerk-users";
const accounts = [
  { localID: "1001", kind: "operator" },
  { localID: "1002", kind: "customer" },
  { localID: "1003", kind: "customer" },
  { localID: "1004", kind: "customer" },
];
const manifest = {
  environment: "development",
  applicationID: "app_3KjGKUGb7dYUMgkjKC0eCWghnOB",
  instanceID: "ins_fixture",
  issuer: "https://fixture.clerk.accounts.dev",
  accounts,
  teams: [
    {
      organizationID: "2001",
      customerID: "3001",
      members: [
        { localID: "1002", role: "owner" },
        { localID: "1003", role: "member" },
      ],
    },
    {
      organizationID: "2002",
      customerID: "3002",
      members: [{ localID: "1004", role: "admin" }],
    },
  ],
};
test("migration requires the exact app and four separate reviewed identities", () => {
  expect(migrationManifest.parse(manifest).accounts).toHaveLength(4);
  for (const change of [
    { applicationID: "app_other" },
    { accounts: accounts.slice(1) },
    { accounts: [accounts[0], accounts[0], accounts[2], accounts[3]] },
  ])
    expect(() => migrationManifest.parse({ ...manifest, ...change })).toThrow();
});
test("operator must not be silently added to a team", () => {
  expect(() =>
    migrationManifest.parse({
      ...manifest,
      teams: [
        { ...manifest.teams[0], members: [{ localID: "1001", role: "admin" }] },
        manifest.teams[1],
      ],
    }),
  ).toThrow();
});

import { migrateClerkUsers } from "../scripts/migrate-clerk-users";
import type { ClerkClient } from "@clerk/backend";
import type { PoolClient } from "pg";
function migrationFixture(unsupportedMailGrant = false, nativeMailSchema = true) {
  const locals = accounts.map((a, i) => ({
    id: a.localID,
    name: "Fixture " + i,
    email: "fixture-" + i + "@example.invalid",
    role: a.kind === "operator" ? "operator" : "user",
    banned: i === 2,
    emailVerified: i !== 3,
  }));
  const userLinks = new Map<
      string,
      { clerk_user_id: string; enabled: boolean }
    >(),
    teamLinks = new Map<string, string>();
  const users: any[] = [],
    orgs: any[] = [],
    members: any[] = [];
  let creates = 0,
    orgCreates = 0,
    writes = 0;
  const db = {
    query: async (sql: string, args: any[]) => {
      if (sql.includes("to_regclass('webdock_mail.mailbox_grant')")) return {rows:[{present:nativeMailSchema}]};
      if (!nativeMailSchema && sql.includes("FROM webdock_mail.mailbox_grant")) throw Error("native mailbox schema absent");
      if (sql.startsWith("SELECT g.mailbox_id FROM webdock_mail.mailbox_grant")) return {rows: unsupportedMailGrant ? [{mailbox_id: "outside-reviewed-team"}] : []};
      if (sql.startsWith("SELECT g.*,t.organization_id,u.role,u.banned,c.status,")) return {rows: []};
      if (sql.includes("SELECT id,name,email,role")) return { rows: locals };
      if (
        sql.includes("SELECT organization_id FROM webdock_auth.tenant_customer")
      )
        return {
          rows: [
            {
              organization_id: manifest.teams.find(
                (t) => t.customerID === args[0],
              )!.organizationID,
            },
          ],
        };
      if (sql.includes('SELECT "userId" AS id,role'))
        return {
          rows: manifest.teams
            .find((t) => t.organizationID === args[0])!
            .members.map((m) => ({ id: m.localID, role: m.role })).concat({id: "1001", role: "owner"}).filter(m => args[1].includes(m.id)),
        };
      if (sql.includes("SELECT clerk_user_id"))
        return { rows: userLinks.has(args[1]) ? [userLinks.get(args[1])] : [] };
      if (sql.startsWith("INSERT INTO webdock_auth.clerk_identity")) {
        writes++;
        if (!userLinks.has(args[2]))
          userLinks.set(args[2], { clerk_user_id: args[1], enabled: args[3] });
        return { rows: [] };
      }
      if (sql.startsWith("SELECT user_id"))
        return {
          rows: [...userLinks]
            .filter(([, v]) => v.clerk_user_id === args[1])
            .map(([id]) => ({ user_id: id })),
        };
      if (sql.startsWith("SELECT name"))
        return { rows: [{ name: "Fixture team " + args[0] }] };
      if (sql.startsWith("SELECT clerk_organization_id"))
        return {
          rows: teamLinks.has(args[1])
            ? [{ clerk_organization_id: teamLinks.get(args[1]) }]
            : [],
        };
      if (sql.startsWith("INSERT INTO webdock_auth.clerk_organization")) {
        writes++;
        if (!teamLinks.has(args[2])) teamLinks.set(args[2], args[1]);
        return { rows: [] };
      }
      if (
        sql.startsWith(
          "SELECT organization_id FROM webdock_auth.clerk_organization",
        )
      )
        return {
          rows: [...teamLinks]
            .filter(([, value]) => value === args[1])
            .map(([organization_id]) => ({ organization_id })),
        };
      throw Error("Unexpected migration query");
    },
  } as unknown as Pick<PoolClient, "query">;
  const provider = {
    instance: { get: async () => ({ id: "ins_fixture", environmentType: "development" }) },
    users: {
      getUser: async (id: string) => users.find((u) => u.id === id),
      getUserList: async (q: any) => ({
        data: users.filter((u) =>
          q.externalId
            ? q.externalId.includes(u.externalId)
            : u.emailAddresses.some((e: any) =>
                q.emailAddress.includes(e.emailAddress),
              ),
        ),
      }),
      createUser: async (input: any) => {
        creates++;
        const u = {
          id: "user_" + creates,
          firstName: input.firstName,
          lastName: input.lastName,
          externalId: input.externalId,
          emailAddresses: input.emailAddress.map(
            (emailAddress: string, i: number) => ({
              emailAddress,
              verification: {
                status:
                  input.emailAddressIdentificationStatus[i] === "verified"
                    ? "verified"
                    : "unverified",
              },
            }),
          ),
        };
        users.push(u);
        return u;
      },
      updateUser: async (id: string, input: any) =>
        Object.assign(
          users.find((u) => u.id === id),
          input,
        ),
    },
    organizations: {
      getOrganization: async (q: any) => {
        const org = orgs.find((o) =>
          q.organizationId ? o.id === q.organizationId : o.slug === q.slug,
        );
        if (!org) throw Object.assign(Error("missing"), { status: 404 });
        return org;
      },
      createOrganization: async (input: any) => {
        const org = { id: "org_" + ++orgCreates, ...input };
        orgs.push(org);
        return org;
      },
      getOrganizationMembershipList: async (q: any) => {
        const data = members.filter(
          (m) => m.organizationId === q.organizationId,
        );
        return { data, totalCount: data.length };
      },
      createOrganizationMembership: async (input: any) => {
        const m = { ...input, publicUserData: { userId: input.userId } };
        members.push(m);
        return m;
      },
      updateOrganizationMembership: async (input: any) =>
        Object.assign(
          members.find(
            (m) =>
              m.organizationId === input.organizationId &&
              m.publicUserData.userId === input.userId,
          ),
          input,
        ),
    },
  } as unknown as ClerkClient;
  return {
    db,
    provider,
    users,
    userLinks,
    members,
    counts: () => ({ creates, orgCreates, writes }),
  };
}
test("dry-run does not provision; repeat apply preserves all four identities, verification and suspension", async () => {
  const f = migrationFixture();
  await migrateClerkUsers(manifest, f.db, f.provider);
  expect(f.counts()).toEqual({ creates: 0, orgCreates: 0, writes: 0 });
  await migrateClerkUsers(manifest, f.db, f.provider, true);
  const links = [...f.userLinks];
  await migrateClerkUsers(manifest, f.db, f.provider, true);
  expect(f.counts().creates).toBe(4);
  expect(f.counts().orgCreates).toBe(2);
  expect([...f.userLinks]).toEqual(links);
  expect(f.members).toHaveLength(3);
  expect(f.userLinks.get("1003")?.enabled).toBe(false);
  expect(f.users[3].emailAddresses[0].verification.status).toBe("unverified");
});
test("duplicate email refuses mapping instead of merging accounts", async () => {
  const f = migrationFixture();
  f.users.push({
    id: "user_conflict",
    externalId: "unrelated",
    emailAddresses: [
      {
        emailAddress: "fixture-0@example.invalid",
        verification: { status: "verified" },
      },
    ],
  });
  await expect(
    migrateClerkUsers(manifest, f.db, f.provider, true),
  ).rejects.toThrow("Email conflict");
  expect(f.counts().creates).toBe(0);
  expect(f.userLinks.size).toBe(0);
});
test('old manifest cannot restore a removed or demoted member',async()=>{
 for(const change of ['remove','demote']){
  const f=migrationFixture();await migrateClerkUsers(manifest,f.db,f.provider,true);
  if(change==='remove')f.members.splice(0,1);else f.members[0].role='org:member';
  for(const apply of [false,true])await expect(migrateClerkUsers(manifest,f.db,f.provider,apply)).rejects.toThrow('must not restore removed access');
  expect(f.counts().creates).toBe(4);expect(f.members.length).toBe(change==='remove'?2:3);
 }
});

 test("migration refuses provider/database environment mismatches before data access", async () => {
 const provider = {instance: {get: async () => ({id: "ins_fixture", environmentType: "development"})}} as unknown as ClerkClient;
 const db = {query: async () => {throw Error("must not query database");}} as unknown as PoolClient;
 await expect(migrateClerkUsers({...manifest, environment:"production"}, db, provider, true, "production")).rejects.toThrow("Provider environment");
 await expect(migrateClerkUsers(manifest, db, provider, true, "production")).rejects.toThrow("Provider environment");
 });

test("an operator mailbox grant outside reviewed teams blocks import before identity writes", async () => {
 const f=migrationFixture(true);
 await expect(migrateClerkUsers(manifest,f.db,f.provider,true)).rejects.toThrow("outside the reviewed teams");
 expect(f.counts()).toEqual({creates:0,orgCreates:0,writes:0});
});

test("optional uninstalled native mailbox schema does not block identity migration", async () => {
 const f=migrationFixture(false,false);
 await migrateClerkUsers(manifest,f.db,f.provider,true);
 expect(f.counts().creates).toBe(4);
});

test("reviewed name fields preserve explicit placeholders without changing identity or roles", async () => {
 const f=migrationFixture();
 const reviewed={...manifest,accounts:manifest.accounts.map((a,i)=>({...a,firstName:i===1?"Elias":undefined,lastName:i===1?"—":undefined}))};
 await migrateClerkUsers(reviewed,f.db,f.provider,true);
 const user=f.users.find(u=>u.externalId==="1002");
 expect(user.firstName).toBe("Elias");expect(user.lastName).toBe("—");
 expect(f.members.find(m=>m.publicUserData.userId===user.id)?.role).toBe("org:admin");
});
