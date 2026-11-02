import { test, expect } from "bun:test";
import { authorizeWebsiteHTTPBinding } from "../src/modules/plugins/website-http";

const scope = {
  subject: "subject-1",
  projectID: "10",
  customerID: "20",
  generation: 2,
  grantRevision: 3,
};
const binding = {
  origin: "https://cms.example.test",
  addresses: ["203.0.113.10"],
  headers: {
    Authorization: "Bearer bridge-secret",
    Cookie: "never-forward",
    "X-Webdock-Role": "operator",
  },
  rules: [
    { method: "GET" as const, path: "/api/webdock/cms", query: ["request"] },
  ],
};
const site = {
  enabled: true,
  customerID: "20",
  redirectURIs: ["https://cms.example.test/api/sso/callback"],
};
const authorize = (role = "editor", value: typeof site | null = site) => ({
  website: async () => value,
  claims: async () => ({ webdock_role: role }),
});
test("scoped website binding derives actor and role and removes stale identity headers and cookies", async () => {
  const result = await authorizeWebsiteHTTPBinding(
    scope,
    "123",
    binding,
    "GET",
    authorize(),
  );
  const headers = new Headers(result.headers);
  expect(headers.get("authorization")).toBe("Bearer bridge-secret");
  expect(headers.get("cookie")).toBeNull();
  expect(headers.get("x-webdock-role")).toBe("editor");
  expect(headers.get("x-webdock-subject")).toBe(scope.subject);
  expect(headers.get("x-webdock-instance")).toBe("123");
  expect(headers.get("x-webdock-project")).toBe(scope.projectID);
  expect(headers.get("x-webdock-grant-epoch")).toBe("2:3");
  expect(binding.headers["X-Webdock-Role"]).toBe("operator");
});
test("reader can read but cannot use a mutation tunnel", async () => {
  await authorizeWebsiteHTTPBinding(
    scope,
    "123",
    binding,
    "GET",
    authorize("reader"),
  );
  await expect(
    authorizeWebsiteHTTPBinding(
      scope,
      "123",
      binding,
      "POST",
      authorize("reader"),
    ),
  ).rejects.toMatchObject({ code: "permission_denied" });
});
test("initial grant revision zero is a valid authority epoch", async () => {
  const result = await authorizeWebsiteHTTPBinding(
    { ...scope, grantRevision: 0 },
    "123",
    binding,
    "GET",
    authorize(),
  );
  expect(result.headers["x-webdock-grant-epoch"]).toBe("2:0");
});
test("cross-customer, disabled, missing and different-origin websites are denied even to operators", async () => {
  for (const value of [
    null,
    { ...site, customerID: "21" },
    { ...site, enabled: false },
    { ...site, redirectURIs: ["https://other.example.test/api/sso/callback"] },
  ]) {
    await expect(
      authorizeWebsiteHTTPBinding(
        scope,
        "123",
        binding,
        "GET",
        authorize("operator", value),
      ),
    ).rejects.toMatchObject({ code: "permission_denied" });
  }
});
test("revoked and malformed identities and invalid epochs fail closed", async () => {
  for (const role of ["", "owner", "EDITOR", "operator\r\nx-evil: yes"]) {
    await expect(
      authorizeWebsiteHTTPBinding(
        scope,
        "123",
        binding,
        "GET",
        authorize(role),
      ),
    ).rejects.toMatchObject({ code: "permission_denied" });
  }
  await expect(
    authorizeWebsiteHTTPBinding(scope, "123", binding, "GET", {
      ...authorize(),
      claims: async () => ({ disabled: true }),
    }),
  ).rejects.toMatchObject({ code: "permission_denied" });
  await expect(
    authorizeWebsiteHTTPBinding(
      { ...scope, generation: 0 },
      "123",
      binding,
      "GET",
      authorize(),
    ),
  ).rejects.toMatchObject({ code: "permission_denied" });
});
test("host action authority requires the exact website alias and gives editors no other service", async () => {
  const { authorizePluginHTTPAction } =
    await import("../src/modules/plugins/website-http");
  const action: any = {
    name: "cms.write",
    intent: "write",
    httpBinding: "cms",
    capabilities: ["plugin.http.request"],
  };
  const project = {
    active: true,
    allowed: true,
    preview: false,
    canWrite: false,
  };
  const deps = {
    ...authorize("editor"),
    resolve: async (name: string) =>
      name === "cms" ? { ...binding, websiteBindingID: "123" } : null,
  };
  expect(
    (
      await authorizePluginHTTPAction(scope, action, project, deps, {
        capability: "plugin.http.request",
        http: { name: "cms", method: "POST" },
      })
    )?.headers["x-webdock-role"],
  ).toBe("editor");
  for (const request of [
    { capability: "plugin.apps.provision" },
    {
      capability: "plugin.http.request",
      http: { name: "analytics", method: "POST" },
    },
  ])
    await expect(
      authorizePluginHTTPAction(scope, action, project, deps, request as any),
    ).rejects.toMatchObject({ code: "permission_denied" });
  await expect(
    authorizePluginHTTPAction(
      scope,
      { ...action, httpBinding: undefined },
      project,
      deps,
    ),
  ).rejects.toMatchObject({ code: "permission_denied" });
  await expect(
    authorizePluginHTTPAction(scope, action, project, {
      ...deps,
      ...authorize("reader"),
    }),
  ).rejects.toMatchObject({ code: "permission_denied" });
  await expect(
    authorizePluginHTTPAction(scope, action, project, {
      ...deps,
      claims: async () => ({ disabled: true }),
    }),
  ).rejects.toMatchObject({ code: "permission_denied" });
  await expect(
    authorizePluginHTTPAction(scope, action, project, {
      ...deps,
      resolve: async () => binding,
    }),
  ).rejects.toMatchObject({ code: "permission_denied" });
});
