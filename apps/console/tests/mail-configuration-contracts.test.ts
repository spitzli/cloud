import assert from "node:assert/strict";
import { test } from "node:test";
import { mailboxCommandSchema, directorySchema } from "../src/mail-contracts";
import {
  configurationObserved,
  configurationRequest,
} from "../src/mail-configuration-client";
const directory = directorySchema.parse({
  data: {
    customerID: "101",
    operator: true,
    canManage: true,
    canConfigure: true,
    configuration: { revision: "1", appliedRevision: "1", state: "succeeded" },
    domains: [
      { id: "301", name: "example.test", state: "ready", dkimKeys: [] },
    ],
    verifiedDomains: [{ id: "201", domain: "example.test" }],
    mailboxes: [],
    people: [],
    grants: [],
  },
}).data;
await test("mailbox creation validates lowercase local parts and exact decimal quota bounds", () => {
  const input = {
    action: "mailbox.create",
    domainID: "301",
    localPart: "support.team",
    name: "Support",
    quotaBytes: "1048576",
  };
  assert.equal(mailboxCommandSchema.parse(input).action, "mailbox.create");
  for (const quotaBytes of ["1048575", "8589934593", "1e9", "1.5", 1048576])
    assert.equal(
      mailboxCommandSchema.safeParse({ ...input, quotaBytes }).success,
      false,
    );
  for (const localPart of ["Support", "a..b", "a@b", "-a", "a-"])
    assert.equal(
      mailboxCommandSchema.safeParse({ ...input, localPart }).success,
      false,
    );
  assert.equal(
    mailboxCommandSchema.safeParse({ ...input, name: "invalid\nname" }).success,
    false,
  );
});
await test("lost creation response resolves only to exact requested mailbox, not unrelated revision progress", () => {
  const input = mailboxCommandSchema.parse({
    action: "mailbox.create",
    domainID: "301",
    localPart: "support",
    name: "Support",
    quotaBytes: "1048576",
  });
  const request = configurationRequest(directory, input, 10);
  const progressed = {
    ...directory,
    configuration: {
      revision: "2",
      appliedRevision: "1",
      state: "pending" as const,
    },
  };
  assert.equal(configurationObserved(request, progressed, 11), false);
  const mailbox = {
    id: "401",
    name: "Support",
    address: "support@example.test",
    quotaBytes: "1048576",
    state: "pending" as const,
    enabled: true,
    canOpen: false,
  };
  assert.equal(
    configurationObserved(request, { ...progressed, mailboxes: [mailbox] }, 10),
    false,
  );
  assert.equal(
    configurationObserved(
      request,
      { ...progressed, mailboxes: [{ ...mailbox, quotaBytes: "2000000" }] },
      11,
    ),
    false,
  );
  assert.equal(
    configurationObserved(request, { ...progressed, mailboxes: [mailbox] }, 11),
    true,
  );
});
await test("revocation observation does not imply remote applied access and retry requires newer revision", () => {
  const current = {
    ...directory,
    grants: [
      { mailboxID: "401", subject: "501", enabled: true, revision: "1" },
    ],
  };
  const revoke = configurationRequest(
    current,
    mailboxCommandSchema.parse({
      action: "revoke",
      mailboxID: "401",
      subject: "501",
    }),
    10,
  );
  assert.equal(
    configurationObserved(
      revoke,
      {
        ...current,
        grants: [
          { mailboxID: "401", subject: "501", revision: "1", enabled: false },
        ],
      },
      11,
    ),
    true,
  );
  const retry = configurationRequest(
    directory,
    mailboxCommandSchema.parse({
      action: "configuration.retry",
      revision: "1",
    }),
    10,
  );
  assert.equal(configurationObserved(retry, directory, 11), false);
});
