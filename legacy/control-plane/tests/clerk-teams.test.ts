import { test, expect } from "bun:test";
import {
  validateMemberChange,
  invitationRole,
} from "../src/modules/auth/clerk-teams-policy";

test("membership constraints are delegated to Clerk, not local admin eligibility", () => {
  const members = [{ id: "admin", role: "org:admin" }];
  for (const action of ["remove", "role"])
    expect(validateMemberChange(members, "admin", action, "member").id).toBe(
      "admin",
    );
});
test("invites default to member and cannot introduce platform roles", () => {
  expect(invitationRole(undefined)).toBe("org:member");
  expect(invitationRole("admin")).toBe("org:admin");
  expect(() => invitationRole("operator")).toThrow();
});
test("native team policy still rejects foreign members, invalid actions and elevated roles", () => {
  const members = [{ id: "member", role: "org:member" }];
  expect(() => validateMemberChange(members, "foreign", "remove")).toThrow();
  expect(() =>
    validateMemberChange(members, "member", "role", "operator"),
  ).toThrow();
  expect(() => validateMemberChange(members, "member", "unknown")).toThrow();
});
