import assert from "node:assert/strict";
import { test } from "node:test";
import {
  serviceSchema,
  directorySchema,
  lifecycleResolved,
} from "../src/mail-contracts";
const value = {
  service: {
    customerID: "101",
    enabled: true,
    state: "ready",
    revision: "9007199254740993",
  },
  mailboxesAvailable: true,
  sendingAvailable: true,
  canActivate: false,
  canSuspend: true,
  canReconcile: false,
  pendingReview: false,
};
await test("Mail preserves decimal revisions and rejects unknown states and numeric revisions", () => {
  assert.equal(
    serviceSchema.parse({ data: value }).data.service.revision,
    "9007199254740993",
  );
  assert.equal(
    serviceSchema.safeParse({
      data: { ...value, service: { ...value.service, revision: 12 } },
    }).success,
    false,
  );
  assert.equal(
    serviceSchema.safeParse({
      data: { ...value, service: { ...value.service, state: "maybe-ready" } },
    }).success,
    false,
  );
});
await test("uncertain lifecycle needs a newer terminal read; failed execution permits explicit recovery", () => {
  const service = serviceSchema.parse({ data: value }).data;
  assert.equal(lifecycleResolved(service, "9007199254740992"), true);
  assert.equal(lifecycleResolved(service, service.service.revision), false);
  assert.equal(
    lifecycleResolved({ ...service, pendingReview: true }, "1"),
    true,
  );
  const failed = {
    ...service,
    pendingReview: true,
    service: { ...service.service, state: "needs_review" as const },
  };
  assert.equal(lifecycleResolved(failed, "1"), true);
  assert.equal(lifecycleResolved(failed, failed.service.revision), false);
  assert.equal(
    lifecycleResolved(
      { ...service, service: { ...service.service, state: "provisioning" } },
      "1",
    ),
    false,
  );
});
await test("configuration awaiting application stays pending independently of service readiness", () => {
  const directory = directorySchema.parse({
    data: {
      customerID: "101",
      operator: true,
      canManage: true,
      canConfigure: true,
      configuration: { revision: "2", appliedRevision: "0", state: "pending" },
      mailboxes: [],
      domains: [],
      verifiedDomains: [],
      people: [],
      grants: [],
    },
  });
  assert.equal(directory.data.configuration?.state, "pending");
  assert.equal(directory.data.configuration.appliedRevision, "0");
});
