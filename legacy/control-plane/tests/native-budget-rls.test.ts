import { expect, test } from "bun:test";
import { buildDatabaseRls } from "../src/db/database-rls-schema";
test("native send ledgers have explicit policies rather than unknown-table fallback", () => {
  const plan = buildDatabaseRls([
    {
      schema: "webdock_mail",
      name: "month_usage",
      columns: ["customer_id", "period_start", "reserved", "accepted"],
    },
    {
      schema: "webdock_mail",
      name: "budget_reservation",
      columns: [
        "reference_id",
        "customer_id",
        "period_start",
        "units",
        "state",
        "send_path",
      ],
    },
  ]);
  expect(plan.unknownTables).toEqual([]);
});
