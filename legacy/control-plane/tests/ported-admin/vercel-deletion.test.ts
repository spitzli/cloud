import assert from "node:assert/strict";
import { test } from "node:test";
import {
  deleteVercelProject,
  VercelAPIError,
} from "../../src/modules/integrations/vercel/vercel-api";

const target = { teamID: "team_test123", projectID: "prj_test123" };
const options = { ...target, token: "test-token" };
const project = { id: target.projectID, accountId: target.teamID };
const code = (expected: string) => (error: unknown) =>
  error instanceof VercelAPIError && error.code === expected;

test("deletion verifies exact project and team before scoped DELETE, accepting only 204", async () => {
  const methods: string[] = [];
  const result = await deleteVercelProject({
    ...options,
    fetcher: async (input, init) => {
      assert.equal(
        String(input),
        `https://api.vercel.com/v9/projects/${target.projectID}?teamId=${target.teamID}`,
      );
      assert.equal(init?.redirect, "error");
      assert.equal(init?.cache, "no-store");
      assert.equal(
        new Headers(init?.headers).get("authorization"),
        "Bearer test-token",
      );
      methods.push(init!.method!);
      return init?.method === "GET"
        ? Response.json(project)
        : new Response(null, { status: 204 });
    },
  });
  assert.deepEqual(methods, ["GET", "DELETE"]);
  assert.deepEqual(result, { status: "deleted" });
});

test("invalid IDs and mismatched journal targets never reach Vercel", async () => {
  for (const changed of [
    { projectID: "name" },
    { projectID: "prj_a/evil" },
    { teamID: "team_a?x=1" },
    { previouslyVerifiedTarget: { ...target, projectID: "prj_other" } },
    { previouslyVerifiedTarget: { ...target, teamID: "team_other" } },
  ]) {
    let calls = 0;
    await assert.rejects(
      deleteVercelProject({
        ...options,
        ...changed,
        fetcher: async () => {
          calls++;
          return Response.json(project);
        },
      }),
    );
    assert.equal(calls, 0);
  }
});

test("wrong or missing ownership never permits DELETE", async () => {
  for (const body of [
    { ...project, id: "prj_other" },
    { ...project, accountId: "team_other" },
    { id: target.projectID },
  ]) {
    await assert.rejects(
      deleteVercelProject({
        ...options,
        fetcher: async (_input, init) => {
          assert.equal(init?.method, "GET");
          return Response.json(body);
        },
      }),
    );
  }
});

test("404 is idempotent only for a matching target previously verified by the service", async () => {
  for (const missingAt of ["GET", "DELETE"]) {
    const fetcher: typeof fetch = async (_input, init) =>
      init?.method === missingAt
        ? new Response(null, { status: 404 })
        : Response.json(project);
    await assert.rejects(
      deleteVercelProject({ ...options, fetcher }),
      code("not_found"),
    );
    assert.deepEqual(
      await deleteVercelProject({
        ...options,
        fetcher,
        previouslyVerifiedTarget: target,
      }),
      { status: "already_absent" },
    );
  }
});

test("permission, upstream and network failures remain explicit and redact provider content", async () => {
  for (const [status, expected] of [
    [401, "disconnected"],
    [403, "deletion_forbidden"],
    [409, "unavailable"],
    [500, "unavailable"],
    [200, "unavailable"],
  ] as const) {
    await assert.rejects(
      deleteVercelProject({
        ...options,
        fetcher: async (_input, init) =>
          init?.method === "GET"
            ? Response.json(project)
            : new Response("private upstream secret", { status }),
      }),
      (error: unknown) =>
        code(expected)(error) &&
        !String(error).includes("private upstream secret"),
    );
  }
  await assert.rejects(
    deleteVercelProject({
      ...options,
      fetcher: async () => {
        throw Error("test-token");
      },
    }),
    code("unavailable"),
  );
  const redirected = new Response(null, { status: 204 });
  Object.defineProperty(redirected, "redirected", { value: true });
  await assert.rejects(
    deleteVercelProject({
      ...options,
      fetcher: async (_input, init) =>
        init?.method === "GET" ? Response.json(project) : redirected,
    }),
    code("unavailable"),
  );
});
