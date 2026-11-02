import { test, expect } from "bun:test";
import {
  databasePlanSchema,
  databasePlanAvailableTo,
  databaseCommandSchema,
} from "@webdock/hosting-contracts";
test("database provisioning sponsorship remains bound to its approved customer", () => {
  const plan = databasePlanSchema.parse({
    kind: "dedicated",
    name: "Sponsored DB",
    monthlyPriceCents: 0,
    currency: "EUR",
    postgresMajor: 18,
    cpuMillicores: 1000,
    memoryBytes: 2147483648,
    volumeBytes: 21474836480,
    logicalStorageAllowanceBytes: null,
    connectionLimit: 20,
    terms: "Approved customer sponsorship; no dedicated backup guarantee.",
    sponsoredCustomerID: "123",
  });
  expect(databasePlanAvailableTo(plan, "123", "pilot")).toBe(true);
  expect(databasePlanAvailableTo(plan, "456", "pilot")).toBe(false);
  expect(databasePlanAvailableTo(plan, "123", "public")).toBe(false);
  expect(
    databasePlanSchema.safeParse({ ...plan, sponsoredCustomerID: undefined })
      .success,
  ).toBe(false);
  expect(
    databasePlanSchema.safeParse({ ...plan, monthlyPriceCents: 1 }).success,
  ).toBe(false);
  expect(
    databaseCommandSchema.safeParse({
      action: "databases.orders.list",
      customerID: "123",
    }).success,
  ).toBe(false);
});
