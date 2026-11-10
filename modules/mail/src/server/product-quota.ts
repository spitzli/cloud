import type { MailProductStore } from "./provider-ports";
import type { DatabaseReader } from "@spitzli/core/identity";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
export function createMailQuota(
  dependencies: Pick<
    MailProductStore,
    "database" | "withMailTransaction" | "assertMailAccess"
  >,
) {
  const { database, withMailTransaction, assertMailAccess } = dependencies;
  async function reserveMailQuota(
    actor: MailActor,
    messageID: string,
    recipientCount: number,
    at: Date,
  ): Promise<void> {
    parseID(messageID);
    if (
      !Number.isInteger(recipientCount) ||
      recipientCount < 1 ||
      recipientCount > 50 ||
      !Number.isFinite(at.getTime())
    )
      throw new MailError(422, "validation_error", "Invalid quota reservation");
    const period = at.toISOString().slice(0, 7) + "-01";
    await withMailTransaction(async (db) => {
      await db.query(
        "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
        [actor.customerID],
      );
      await assertMailAccess(actor, "send", db);
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "mail-budget:" + actor.customerID,
      ]);
      const message = (
        await db.query(
          "SELECT id,simulated,dispatch_status FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3 FOR UPDATE",
          [messageID, actor.customerID, actor.connectionID],
        )
      ).rows[0];
      if (!message) mailUnavailable();
      if (message.simulated) return;
      const existing = (
        await db.query(
          "SELECT recipients,status FROM webdock_auth.mail_usage_reservation WHERE message_id=$1",
          [messageID],
        )
      ).rows[0];
      if (existing) {
        if (existing.recipients !== recipientCount) mailUnavailable();
        return;
      }
      if (!["scheduled", "dispatching"].includes(message.dispatch_status))
        mailUnavailable();
      const actual = Number(
        (
          await db.query(
            "SELECT count(*) FROM webdock_auth.mail_recipient WHERE message_id=$1 AND suppression_reason IS NULL",
            [messageID],
          )
        ).rows[0].count,
      );
      if (actual !== recipientCount) mailUnavailable();
      const budget = await assertQuotaCapacity(
        db,
        actor.customerID,
        recipientCount,
        at,
      );
      await db.query(
        "INSERT INTO webdock_auth.mail_usage_period(customer_id,connection_id,period,budget,reserved) VALUES($1,$2,$3,$4,$5) ON CONFLICT(customer_id,connection_id,period) DO UPDATE SET reserved=mail_usage_period.reserved+$5,budget=$4",
        [
          actor.customerID,
          actor.connectionID,
          period,
          budget?.toString() ?? null,
          recipientCount,
        ],
      );
      await db.query(
        "INSERT INTO webdock_auth.mail_usage_reservation(customer_id,connection_id,message_id,period,recipients,status) VALUES($1,$2,$3,$4,$5,'reserved')",
        [
          actor.customerID,
          actor.connectionID,
          messageID,
          period,
          recipientCount,
        ],
      );
    });
  }
  /** Internal settlement; unknown provider acceptance never releases a reservation. */
  async function settleMailQuota(
    messageID: string,
    outcome: "accepted" | "rejected" | "unknown",
    transaction?: DatabaseReader,
  ): Promise<void> {
    parseID(messageID);
    if (outcome === "unknown") return;
    if (!["accepted", "rejected"].includes(outcome))
      throw new MailError(422, "validation_error", "Invalid quota outcome");
    const customer = (
      await (transaction ?? database).query(
        "SELECT customer_id FROM webdock_auth.mail_usage_reservation WHERE message_id=$1",
        [messageID],
      )
    ).rows[0];
    if (!customer) return;
    const run = async (db: DatabaseReader) => {
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "mail-budget:" + customer.customer_id,
      ]);
      const row = (
        await db.query(
          "UPDATE webdock_auth.mail_usage_reservation SET status=$2 WHERE message_id=$1 AND status='reserved' RETURNING *",
          [messageID, outcome === "accepted" ? "consumed" : "released"],
        )
      ).rows[0];
      if (!row) return;
      await db.query(
        "UPDATE webdock_auth.mail_usage_period SET reserved=reserved-$4,consumed=consumed+$5 WHERE customer_id=$1 AND connection_id=$2 AND period=$3",
        [
          row.customer_id,
          row.connection_id,
          row.period,
          row.recipients,
          outcome === "accepted" ? row.recipients : 0,
        ],
      );
    };
    if (transaction) await run(transaction);
    else await withMailTransaction(run);
  }

  async function assertQuotaCapacity(
    db: DatabaseReader,
    customerID: string,
    recipientCount: number,
    at: Date,
  ): Promise<bigint | null> {
    const period = at.toISOString().slice(0, 7) + "-01";
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "mail-budget:" + customerID,
    ]);
    const subscription = (
      await db.query(
        "SELECT base,extras FROM webdock_auth.tenant_subscription WHERE customer_id=$1 FOR SHARE",
        [customerID],
      )
    ).rows[0];
    const settings = (
      await db.query(
        "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
      )
    ).rows[0]?.settings;
    const base = subscription
        ? subscription.base.mailMessages
        : (settings?.mailDefaultLimit ?? 1000),
      extra = subscription?.extras?.mailMessages ?? 0;
    if (
      (base !== null && (!Number.isSafeInteger(base) || base < 0)) ||
      !Number.isSafeInteger(extra) ||
      extra < 0
    )
      throw new MailError(
        503,
        "quota_unavailable",
        "Mail quota configuration is unavailable",
      );
    const budget = base === null ? null : BigInt(base) + BigInt(extra);
    const usage = (
      await db.query(
        process.env.DATABASE_RLS_SECRET
          ? "SELECT webdock_rls.mail_usage_total($1,$2) AS used"
          : "SELECT coalesce(sum(reserved+consumed),0)::text AS used FROM webdock_auth.mail_usage_period WHERE customer_id=$1 AND period=$2",
        [customerID, period],
      )
    ).rows[0];
    const native =
      !process.env.DATABASE_RLS_SECRET &&
      (
        await db.query(
          "SELECT to_regclass('webdock_mail.month_usage') AS present",
        )
      ).rows[0].present
        ? BigInt(
            (
              await db.query(
                "SELECT coalesce(sum(reserved+accepted),0)::text AS used FROM webdock_mail.month_usage WHERE customer_id=$1 AND period_start=$2",
                [customerID, period],
              )
            ).rows[0].used,
          )
        : 0n;
    if (
      budget !== null &&
      BigInt(usage.used) + native + BigInt(recipientCount) > budget
    )
      throw new MailError(
        429,
        "quota_exceeded",
        "Monthly recipient allowance exhausted",
      );
    return budget;
  }
  /** Repair a definitive terminal row left by an older worker crashing between completion and settlement. */
  async function recoverTerminalQuotas(limit = 100): Promise<number> {
    if (!Number.isInteger(limit) || limit < 1 || limit > 100)
      throw new MailError(422, "invalid_limit", "Invalid recovery limit");
    const candidates = (
      await database.query(
        `SELECT m.id,m.customer_id FROM webdock_auth.mail_usage_reservation r JOIN webdock_auth.mail_message m ON m.id=r.message_id WHERE r.status='reserved' AND m.dispatch_status IN ('accepted','failed','canceled','suppressed') ORDER BY m.id LIMIT $1`,
        [limit],
      )
    ).rows;
    let recovered = 0;
    for (const candidate of candidates)
      await withMailTransaction(async (db) => {
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          "mail-budget:" + candidate.customer_id,
        ]);
        const message = (
          await db.query(
            "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1 AND dispatch_status IN ('accepted','failed','canceled','suppressed') FOR UPDATE",
            [candidate.id],
          )
        ).rows[0];
        if (!message) return;
        await settleMailQuota(
          candidate.id,
          message.dispatch_status === "accepted" ? "accepted" : "rejected",
          db,
        );
        recovered++;
      });
    return recovered;
  }

  return {
    reserveMailQuota,
    settleMailQuota,
    assertQuotaCapacity,
    recoverTerminalQuotas,
  };
}
