import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createWorkerDatabases } from "../src/db/client";
import { assertMailCanDisable } from "@webdock/module-mail/server/lifecycle";
(process.env.WEBDOCK_RLS_FIXTURE_FILE ? test : test.skip)(
  "Mail shutdown sees retained suspended instances through the restricted worker role",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Disposable restricted-role fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const pools = createWorkerDatabases({
      environment: "development",
      workerURL: process.env.DATABASE_WORKER_URL!,
      signingSecret: process.env.DATABASE_RLS_SECRET,
      max: 1,
    });
    try {
      const customer = (
        await owner.query(
          "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Module preflight fixture') RETURNING id",
        )
      ).rows[0].id;
      await owner.query(
        "INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,state) VALUES($1,$2,'fixture','suspended')",
        [customer, randomUUID()],
      );
      await withDatabaseScope(
        { kind: "system", purpose: "mail-cron" },
        async () => {
          const row = (
            await pools.worker.query(
              "SELECT state FROM webdock_mail.service WHERE customer_id=$1",
              [customer],
            )
          ).rows[0];
          expect(row?.state).toBe("suspended");
          await expect(assertMailCanDisable(pools.worker)).rejects.toThrow(
            "retained",
          );
        },
      );
    } finally {
      await owner.end();
      await pools.close();
    }
  },
);
