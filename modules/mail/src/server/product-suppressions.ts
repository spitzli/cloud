import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseAddress } from "@spitzli/mail-contracts/validation";
import { MailError } from "./product-access";
export function createMailSuppressions(
  dependencies: Pick<
    MailProductStore,
    "database" | "withMailTransaction" | "assertMailAccess"
  >,
) {
  const { database, withMailTransaction, assertMailAccess } = dependencies;
  async function getSuppressedRecipients(
    customerID: string,
    emails: string[],
  ): Promise<Set<string>> {
    const result = await database.query(
      "SELECT email FROM webdock_auth.mail_suppression WHERE customer_id=$1 AND email=ANY($2)",
      [customerID, emails],
    );
    return new Set(result.rows.map((r) => r.email as string));
  }
  async function setMailSuppression(
    actor: MailActor,
    input: { email: string; reason: string },
  ): Promise<void> {
    const email = parseAddress(input.email);
    if (
      !["manual", "bounced", "complained", "unsubscribed"].includes(
        input.reason,
      )
    )
      throw new MailError(
        422,
        "validation_error",
        "Invalid suppression reason",
      );
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      await db.query(
        "INSERT INTO webdock_auth.mail_suppression(customer_id,email,reason,source) VALUES($1,$2,$3,'application') ON CONFLICT(customer_id,email) DO UPDATE SET reason=excluded.reason,source=excluded.source WHERE mail_suppression.reason NOT IN ('complained','unsubscribed') OR excluded.reason IN ('complained','unsubscribed')",
        [actor.customerID, email, input.reason],
      );
    });
  }
  async function removeMailSuppression(
    actor: MailActor,
    email: string,
  ): Promise<void> {
    const normalized = parseAddress(email);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      await db.query(
        "DELETE FROM webdock_auth.mail_suppression WHERE customer_id=$1 AND email=$2",
        [actor.customerID, normalized],
      );
    });
  }
  async function listMailSuppressions(actor: MailActor) {
    await assertMailAccess(actor, "read");
    return (
      await database.query(
        "SELECT email,reason,source,created_at FROM webdock_auth.mail_suppression WHERE customer_id=$1 ORDER BY email LIMIT 100",
        [actor.customerID],
      )
    ).rows;
  }

  return {
    getSuppressedRecipients,
    setMailSuppression,
    removeMailSuppression,
    listMailSuppressions,
  };
}
