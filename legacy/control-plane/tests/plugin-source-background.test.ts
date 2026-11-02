import { test, expect } from "bun:test";
import { existsSync } from "node:fs";
import {
  currentDatabaseScope,
  withDatabaseScope,
} from "@webdock/database-runtime";
import * as identities from "../src/modules/plugins/sources-identity";
const dispatcherFile = new URL(
  "../src/modules/plugins/sources-dispatch.ts",
  import.meta.url,
);
const dispatchers = existsSync(dispatcherFile)
  ? await import(dispatcherFile.pathname)
  : null;

test("installer claim activates one ready import under verified Auth scope without a browser", async () => {
  expect(dispatchers).not.toBeNull();
  const events: string[] = [];
  const scope = {
    kind: "session" as const,
    subject: "auth-subject",
    sessionID: "fresh-session",
  };
  const job = {
    id: "1",
    actor_scope: scope,
    project_id: "2",
    customer_id: "3",
  };
  const dispatch = dispatchers.createPluginInstallerDispatch({
    nextReady: async () => {
      events.push("selection-complete");
      return job;
    },
    withActor: async (value: any, work: () => Promise<unknown>) => {
      expect(value).toBe(job);
      events.push("verified");
      return withDatabaseScope(scope, work);
    },
    activate: async (id: string) => {
      expect(id).toBe("1");
      expect(currentDatabaseScope()).toEqual(scope);
      events.push("activated");
    },
    worker: async (action: string) => {
      expect(action).toBe("claim");
      expect(currentDatabaseScope()?.kind).toBe("system");
      events.push("claimed");
      return null;
    },
  });
  expect(
    await withDatabaseScope(
      { kind: "system", purpose: "plugin-controller" },
      () => dispatch("claim", {}),
    ),
  ).toBeNull();
  expect(events).toEqual([
    "selection-complete",
    "verified",
    "activated",
    "claimed",
  ]);
});

test("temporary readiness and rejected stored authority leave activation pending and continue claiming", async () => {
  expect(dispatchers).not.toBeNull();
  for (const failure of ["verify", "probe"]) {
    let selections = 0,
      activations = 0,
      claims = 0;
    const dispatch = dispatchers.createPluginInstallerDispatch({
      nextReady: async () => {
        selections++;
        return { id: "1" };
      },
      withActor: async (_job: any, work: () => Promise<unknown>) => {
        if (failure === "verify") throw Error("revoked");
        return work();
      },
      activate: async () => {
        activations++;
        throw Object.assign(Error("not ready"), { status: 503 });
      },
      worker: async () => {
        claims++;
        return { id: "next-build" };
      },
    });
    expect(await dispatch("claim", {})).toEqual({ id: "next-build" });
    expect(selections).toBe(1);
    expect(activations).toBe(failure === "verify" ? 0 : 1);
    expect(claims).toBe(1);
    await expect(dispatch("claim", { unexpected: true })).rejects.toThrow();
    expect(selections).toBe(1);
  }
});

test("stored source sessions are revalidated before work and Clerk authority is freshly restored", async () => {
  const factory = (identities as any).createVerifiedSourceActorRunner;
  expect(typeof factory).toBe("function");
  const scope = {
    kind: "session" as const,
    subject: "auth-subject",
    sessionID: "session",
    clerk: {
      issuer: "https://clerk.example",
      userID: "provider-user",
      clerkOrganizationID: null,
      permissions: ["old"],
    },
  };
  let authorized = 0,
    executed = 0;
  const principal = { userID: "auth-subject", sessionID: "session" };
  const deps = {
    revalidate: async () => principal,
    withPrincipal: (_principal: any, work: () => Promise<unknown>) =>
      withDatabaseScope(
        { ...scope, clerk: { ...scope.clerk, permissions: ["fresh"] } } as any,
        work,
      ),
    authorize: async () => {
      authorized++;
      expect((currentDatabaseScope() as any).clerk.permissions).toEqual([
        "fresh",
      ]);
    },
  };
  const run = factory(deps);
  await run(scope, "project", "customer", async () => {
    executed++;
    expect((currentDatabaseScope() as any).clerk.permissions).toEqual([
      "fresh",
    ]);
  });
  expect(authorized).toBe(1);
  expect(executed).toBe(1);
  await expect(
    factory({
      ...deps,
      revalidate: async () => ({ ...principal, userID: "changed" }),
    })(scope, "project", "customer", async () => executed++),
  ).rejects.toThrow();
  expect(executed).toBe(1);
  await expect(
    factory({
      ...deps,
      authorize: async () => {
        throw Error("SQL session expired");
      },
    })(
      { kind: "session", subject: "auth-subject", sessionID: "expired" },
      "project",
      "customer",
      async () => executed++,
    ),
  ).rejects.toThrow();
  expect(executed).toBe(1);
});
