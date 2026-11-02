import { test, expect } from "bun:test";
import { studioOperationDomain } from "../src/modules/portal/operation-domain";
test("managed databases and Mail retain compatibility commands without requiring Hosting ownership", () => {
  for (const action of ["databases.permissions", "databases.offer", "databases.acceptOrder", "databases.list"])
    expect(studioOperationDomain("hosting.command", [{ action }])).toBe("databases");
  expect(studioOperationDomain("database.command", [{ action: "open" }])).toBe("databases");
  expect(studioOperationDomain("hosting.command", [{ action: "byok.mail.domain" }])).toBe("mail");
  expect(studioOperationDomain("hosting.command", [{ action: "apps.deploy" }])).toBe("hosting");
  expect(studioOperationDomain("session", [])).toBe("auth");
  expect(studioOperationDomain("project.remove", [])).toBe("lifecycle");
});
