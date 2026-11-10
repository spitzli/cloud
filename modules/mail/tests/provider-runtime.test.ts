import { test } from "node:test";
import assert from "node:assert/strict";
import type { DatabaseReader } from "@spitzli/core/identity";
import type { MailActor } from "@spitzli/mail-contracts";
import { createMailCredentials } from "../src/server/provider-credentials.ts";
import {
  createTenantMail,
  TenantMailError,
} from "../src/server/tenant-mail.ts";
import type { MailAccountProvider } from "../src/server/provider-ports.ts";

const actor: MailActor = {
  customerID: "123",
  connectionID: "456",
  principal: { kind: "session", userID: "789", sessionID: "session" },
};
function credentialFixture(
  options: {
    uncertain?: boolean;
    mismatch?: boolean;
    denied?: boolean;
    pending?: boolean;
  } = {},
) {
  const queries: string[] = [],
    contexts: string[] = [],
    authorizations: string[] = [];
  let issued = 0;
  const database = {
    query: async (sql: string) => {
      queries.push(sql);
      let rows: unknown[] = [];
      if (sql.startsWith("SELECT * FROM webdock_auth.mail_connection"))
        rows = [
          {
            id: "456",
            kind: "platform",
            state: "ready",
            provider_account_id: "bound-account",
            revision: "8",
          },
        ];
      else if (
        sql.includes("mail_provider_operation") &&
        sql.startsWith("SELECT")
      )
        rows = options.pending ? [{}] : [];
      else if (sql.startsWith("SELECT provider_id,email,snapshot"))
        rows = [
          {
            provider_id: "bound-account",
            email: "bound@example.org",
            snapshot: { active: true },
          },
        ];
      else if (sql.startsWith("SELECT coalesce")) rows = [{ version: 3 }];
      else if (sql.startsWith("SELECT id FROM webdock_auth.mail_connection"))
        rows = [{ id: "456" }];
      else if (sql.startsWith("SELECT provider_id FROM"))
        rows = [{ provider_id: "bound-account" }];
      return { rows, rowCount: rows.length };
    },
  } as unknown as DatabaseReader;
  const runtime = createMailCredentials({
    database,
    withMailTransaction: async (run) => run(database),
    assertMailAccess: async (_actor, capability, db) => {
      authorizations.push(capability);
      assert.equal(db, database);
      if (options.denied) throw Error("denied");
    },
    encryptMailSecret: (_value, context) => {
      contexts.push(context);
      return "sealed";
    },
    decryptMailSecret: () => {
      throw Error("No old credentials in fixture");
    },
    master: async (boundActor) => {
      assert.equal(boundActor, actor);
      return {
        getSubaccount: async (id) => {
          assert.equal(id, "bound-account");
          return {
            id: options.mismatch ? "other-account" : id,
            email: "bound@example.org",
            active: true,
          };
        },
        authorizeSubaccount: async (email) => {
          assert.equal(email, "bound@example.org");
          return "scoped-token";
        },
      };
    },
    child: (token) => {
      assert.equal(token, "scoped-token");
      return {
        createConsumerKey: async (_token, _label, scope) => {
          issued++;
          assert.deepEqual(scope, { permissions: ["SEND_API"], ips: [] });
          if (options.uncertain) throw Error("timeout after provider accepted");
          return { consumerKey: "key", consumerSecret: "secret" };
        },
      };
    },
  });
  return {
    runtime,
    queries,
    contexts,
    authorizations,
    get issued() {
      return issued;
    },
  };
}
test("credential module preserves bound account, transaction authorization and encryption context", async () => {
  const f = credentialFixture();
  await f.runtime.ensureManagedSendingCredential(actor);
  assert.equal(f.issued, 1);
  assert.deepEqual(f.authorizations, ["operate", "operate"]);
  assert.deepEqual(f.contexts, ["mail-send:123:456:3"]);
  assert(f.queries.some((q) => q.includes("status='succeeded'")));
});
test("uncertain credential creation retains a reconciliation fence", async () => {
  const f = credentialFixture({ uncertain: true });
  await assert.rejects(f.runtime.ensureManagedSendingCredential(actor), {
    code: "needs_review",
  });
  assert.equal(f.issued, 1);
  assert(f.queries.some((q) => q.includes("status='needs_review'")));
  assert(
    !f.queries.some((q) =>
      q.startsWith("INSERT INTO webdock_auth.mail_connection_secret"),
    ),
  );
});
test("pending operations and account mismatch never create a new credential", async () => {
  for (const options of [{ pending: true }, { mismatch: true }]) {
    const f = credentialFixture(options);
    await assert.rejects(f.runtime.ensureManagedSendingCredential(actor));
    assert.equal(f.issued, 0);
  }
});
test("denied access never reserves or calls a credential provider", async () => {
  const f = credentialFixture({ denied: true });
  await assert.rejects(
    f.runtime.ensureManagedSendingCredential(actor),
    /denied/,
  );
  assert.equal(f.issued, 0);
  assert.deepEqual(f.queries, []);
});
test("tenant account timeout preserves reservation while definitive rejection releases it", async () => {
  for (const definite of [false, true]) {
    const queries: string[] = [];
    const database = {
      query: async (sql: string) => {
        queries.push(sql);
        return {
          rows: sql.includes("RETURNING id") ? [{ id: "audit" }] : [],
          rowCount: 1,
        };
      },
    } as unknown as DatabaseReader;
    const provider: MailAccountProvider = {
      createSubaccount: async () => {
        throw Error("provider failure");
      },
      getSubaccount: async () => {
        throw Error("unexpected");
      },
      getSubaccountPlan: async () => {
        throw Error("unexpected");
      },
      listSubaccounts: async () => {
        throw Error("unexpected");
      },
      setSubaccountActive: async () => {
        throw Error("unexpected");
      },
      setSubaccountLimit: async () => {
        throw Error("unexpected");
      },
    };
    const runtime = createTenantMail({
      database,
      withAuthenticatedDatabase: async (_headers, run) => run(),
      getTenant: async () => ({
        tenant: { id: "123", name: "Tenant", status: "active" },
        operator: true,
        canManage: true,
      }),
      resolveSessionReference: async () => ({
        subject: "789",
        sessionID: "session",
      }),
      getPlatformSettings: async () => ({
        mailEnabled: true,
        mailRegion: "eu",
        mailDefaultLimit: 100,
        mailSendingIP: "192.0.2.1",
      }),
      getMailConnectionStatus: async () => ({ configured: true }),
      encryptMailSecret: (_value, context) => {
        assert.equal(context, "123");
        return "sealed";
      },
      requireMail: (intent) => {
        assert.equal(intent, "provision");
      },
      provider: async () => provider,
      definitelyRejectedCreation: () => definite,
    });
    await assert.rejects(
      runtime.manageTenantMail(new Headers(), "123", {
        action: "provision",
        email: "owner@example.org",
        firstName: "First",
        lastName: "Last",
        policyAgree: "yes",
      }),
      TenantMailError,
    );
    assert.equal(
      queries.some((q) =>
        q.startsWith("DELETE FROM webdock_auth.mail_tenant_account"),
      ),
      definite,
    );
    assert.equal(
      queries.some((q) => q.includes("SET state='needs_review'")),
      !definite,
    );
  }
});
