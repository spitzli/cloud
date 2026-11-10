import type { MailActor, MailCapability } from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { MailError } from "@spitzli/mail-contracts/errors";
import {
  activeIdentity,
  type IdentityDirectory,
  type DatabaseReader,
  type SessionReference,
} from "@spitzli/core/identity";
export { MailError } from "@spitzli/mail-contracts/errors";
export function mailUnavailable(): never {
  throw new MailError(404, "not_found", "Mail resource unavailable");
}
export function createMailAuthorization(dependencies: {
  identity: IdentityDirectory;
  byokAllowed: (db: DatabaseReader, customerID: string) => Promise<boolean>;
}) {
  async function assertMailAccess(
    actor: MailActor,
    capability: MailCapability,
    db: DatabaseReader,
    admission = true,
  ): Promise<void> {
    parseID(actor.customerID);
    parseID(actor.connectionID);
    const connection = (
      await db.query(
        `SELECT m.*,c.status AS customer_status,p.settings FROM webdock_auth.mail_connection m
 JOIN webdock_admin.customers c ON c.id=m.customer_id LEFT JOIN webdock_auth.platform_settings p ON p.id=true
 WHERE m.id=$1 AND m.customer_id=$2`,
        [actor.connectionID, actor.customerID],
      )
    ).rows[0];
    if (!connection || connection.customer_status !== "active")
      mailUnavailable();
    const principal = actor.principal;
    if (principal.kind === "session") {
      const user = await dependencies.identity.session(db, {
        subject: principal.userID,
        sessionID: principal.sessionID,
      });
      if (
        !activeIdentity(user) ||
        !user ||
        user.subject !== principal.userID ||
        user.sessionID !== principal.sessionID ||
        user.preview
      )
        mailUnavailable();
      const membership = await dependencies.identity.membership(
        db,
        user.subject,
        actor.customerID,
      );
      if (!membership) mailUnavailable();
      if (user.role === "operator") {
        await db.query(
          "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'succeeded')",
          [principal.userID, "mail-product-" + capability, actor.customerID],
        );
      } else {
        if (
          !membership.memberID ||
          !["member", "admin", "owner"].includes(membership.role ?? "")
        )
          mailUnavailable();
        if (
          capability === "operate" ||
          (capability === "manage" &&
            !["admin", "owner"].includes(membership.role ?? ""))
        )
          mailUnavailable();
      }
    } else if (principal.kind === "api_key") {
      const key = (
        await db.query(
          "SELECT permission FROM webdock_auth.mail_api_key WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND revoked_at IS NULL",
          [principal.keyID, actor.customerID, actor.connectionID],
        )
      ).rows[0];
      if (
        !key ||
        capability === "operate" ||
        (capability !== "send" && key.permission !== "full_access")
      )
        mailUnavailable();
    } else if (principal.kind === "job") {
      // Jobs are bound to a live, fenced claim, not a client-supplied actor flag.
      const claim = (
        await db.query(
          "SELECT 1 FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND dispatch_status='dispatching' AND lease_until>now()",
          [principal.jobID, actor.customerID, actor.connectionID],
        )
      ).rowCount;
      if (!claim || capability !== "send") mailUnavailable();
    } else mailUnavailable();
    if (capability !== "read" && admission) {
      if (connection.settings?.mailGatewayEnabled !== true)
        throw new MailError(
          503,
          "gateway_disabled",
          "Mail gateway is disabled",
        );
      if (connection.kind === "byok") {
        if (!(await dependencies.byokAllowed(db, actor.customerID)))
          mailUnavailable();
      }
      if (
        connection.kind === "platform" &&
        connection.settings?.mailEnabled !== true
      )
        throw new MailError(503, "mail_disabled", "Platform Mail is disabled");
      if (
        connection.deleted_at ||
        (capability === "send" && connection.state !== "ready")
      )
        throw new MailError(
          409,
          "connection_unavailable",
          "Sending connection is not ready",
        );
    }
  }

  async function assertMailManager(
    reference: SessionReference,
    customerID: string,
    db: DatabaseReader,
  ) {
    parseID(customerID);
    const user = await dependencies.identity.session(db, reference, {
      lock: true,
    });
    if (
      !activeIdentity(user) ||
      !user ||
      user.subject !== reference.subject ||
      user.sessionID !== reference.sessionID ||
      user.preview
    )
      mailUnavailable();
    const customer = (
      await db.query(
        "SELECT status FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
        [customerID],
      )
    ).rows[0];
    if (customer?.status !== "active") mailUnavailable();
    const membership = await dependencies.identity.membership(
      db,
      user.subject,
      customerID,
      { lock: true },
    );
    if (
      !membership ||
      (user.role !== "operator" &&
        (!membership.memberID ||
          !["owner", "admin"].includes(membership.role ?? "")))
    )
      mailUnavailable();
    return { subject: user.subject, operator: user.role === "operator" };
  }
  return { assertMailAccess, assertMailManager };
}
