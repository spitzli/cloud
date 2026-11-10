import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
export function createMailConnections(
  dependencies: Pick<
    MailProductStore,
    "database" | "withMailTransaction" | "assertMailAccess"
  >,
) {
  const { database, withMailTransaction, assertMailAccess } = dependencies;
  async function getMailConnections(actor: MailActor) {
    await assertMailAccess(actor, "read");
    return (
      await database.query(
        `SELECT c.id,c.kind,c.region,c.state,c.revision,s.key_suffix FROM webdock_auth.mail_connection c
 LEFT JOIN webdock_auth.mail_connection_secret s ON s.connection_id=c.id AND s.active
 WHERE c.customer_id=$1 AND ($2::text IS NULL OR c.id=$2) ORDER BY c.created_at,c.id`,
        [
          actor.customerID,
          actor.principal.kind === "api_key" ? actor.connectionID : null,
        ],
      )
    ).rows;
  }
  async function pauseMailConnection(actor: MailActor): Promise<void> {
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      await db.query(
        "UPDATE webdock_auth.mail_connection SET state='paused',revision=revision+1,updated_at=now() WHERE id=$1 AND customer_id=$2",
        [actor.connectionID, actor.customerID],
      );
    });
  }

  return { getMailConnections, pauseMailConnection };
}
