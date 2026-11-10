import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
import { MailError } from "./product-access";
export function createMailRateLimit(
  dependencies: Pick<
    MailProductStore,
    "withMailTransaction" | "assertMailAccess"
  >,
) {
  const { withMailTransaction, assertMailAccess } = dependencies;
  async function consumeMailRate(
    actor: MailActor,
    input: { requests: number; recipients: number; keys: number },
  ): Promise<void> {
    for (const n of [input.requests, input.recipients, input.keys])
      if (!Number.isSafeInteger(n) || n < 0)
        throw new MailError(422, "validation_error", "Invalid rate units");
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "send", db, false);
      for (const [kind, limit, unit] of [
        ["requests", 120, "minute"],
        ["recipients", 1000, "minute"],
        ["keys", 10, "hour"],
      ] as const) {
        const amount = input[kind];
        if (!amount) continue;
        const result = await db.query(
          `INSERT INTO webdock_auth.mail_rate_window(customer_id,kind,window_start,count)
    SELECT $1,$2,date_trunc($3,now()),$4::integer WHERE $4::integer<=$5::integer
    ON CONFLICT(customer_id,kind,window_start) DO UPDATE SET count=mail_rate_window.count+$4
    WHERE mail_rate_window.count+$4<=$5 RETURNING count`,
          [actor.customerID, kind, unit, amount, limit],
        );
        if (!result.rowCount)
          throw new MailError(
            429,
            "rate_limit_exceeded",
            "Mail request limit exceeded",
          );
      }
    });
  }

  return { consumeMailRate };
}
