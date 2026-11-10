import type { NativeMailPorts } from "./native-ports";
import { MailServiceError } from "./instances";
import {
  activeIdentity,
  type IdentityDirectory,
  type LocalIdentity,
  type DatabaseReader as Connection,
} from "@spitzli/core/identity";
export type MailActor = { subject: string; sessionID: string };
export function mailUnavailable(): never {
  throw new MailServiceError("Mail access is unavailable.");
}
export function createNativeMailAuthorization(identity: IdentityDirectory, clerkMailAccess: NativeMailPorts["clerkMailAccess"] = async () => undefined) {
  function validUser(user: LocalIdentity | null) {
    if (!activeIdentity(user)) mailUnavailable();
    return user;
  }
  async function subjectAccess(
    db: Connection,
    user: LocalIdentity,
    customerID: string,
    manage: boolean,
    requireReady: boolean,
  ) {
    if (!/^[1-9][0-9]{0,18}$/.test(customerID)) mailUnavailable();
    const tenant = (
      await db.query(
        `SELECT c.status FROM webdock_admin.customers c WHERE c.id=$1 FOR SHARE OF c`,
        [customerID],
      )
    ).rows[0];
    if (!tenant || tenant.status !== "active") mailUnavailable();
    const clerk = await clerkMailAccess(db, user.subject, customerID);
    const member = clerk === undefined ? await identity.membership(db, user.subject, customerID, {
      lock: true,
    }) : null;
    const operator = clerk ? clerk.operator : user.role === "operator";
    if (clerk === null || (!clerk && !operator && (!member?.role || !member.memberID))) mailUnavailable();
    const canManage = clerk ? clerk.canManage : operator || ["owner", "admin"].includes(member?.role ?? "");
    if (manage && !canManage) mailUnavailable();
    const service = (
      await db.query(
        `SELECT s.*,s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') AS ready FROM webdock_mail.service s WHERE customer_id=$1 FOR SHARE`,
        [customerID],
      )
    ).rows[0];
    if (requireReady && !service?.ready) mailUnavailable();
    return {
      operator,
      canManage,
      service,
      memberID: clerk ? null : member?.memberID ?? null,
      clerkGrant: clerk?.clerkGrant ?? null,
    };
  }
  /** Browser management and content always require a live authenticated session. */
  async function authorizeMail(
    db: Connection,
    actor: MailActor,
    customerID: string,
    manage = false,
    requireReady = true,
  ) {
    if (!actor.sessionID) mailUnavailable();
    const principal = await identity.session(db, actor, { lock: true });
    if (
      !principal ||
      principal.subject !== actor.subject ||
      principal.sessionID !== actor.sessionID ||
      principal.preview
    )
      mailUnavailable();
    const user = validUser(principal);
    return subjectAccess(db, user, customerID, manage, requireReady);
  }
  async function mailboxAccess(
    db: Connection,
    access: Awaited<ReturnType<typeof subjectAccess>>,
    subject: string,
    customerID: string,
    mailboxID: string,
  ) {
    // An operator's role never substitutes for an explicit mailbox grant.
    const row = (
      await db.query(
        `SELECT b.id,b.name,b.provider_id AS "accountID",d.name AS domain,b.local_part AS "localPart",i.id AS "identityID",i.provider_id AS "principalID",g.revision AS "grantRevision"
 FROM webdock_mail.mailbox b JOIN webdock_mail.domain d ON d.id=b.domain_id AND d.customer_id=b.customer_id
 JOIN webdock_mail.mailbox_grant g ON g.mailbox_id=b.id AND g.customer_id=b.customer_id AND g.subject=$3
 JOIN webdock_mail.identity i ON i.customer_id=g.customer_id AND i.subject=g.subject
 WHERE b.id=$1 AND b.customer_id=$2 AND b.enabled AND g.enabled AND g.revision=g.applied_revision AND b.state='ready' AND i.state='ready' AND b.provider_id IS NOT NULL AND i.provider_id IS NOT NULL AND ((g.clerk_grant=$6) OR (g.clerk_grant IS NULL AND $6::text IS NULL AND (g.member_id=$4 OR (g.operator_grant AND $5)))) FOR SHARE OF b,g,i`,
      [
        mailboxID,
        customerID,
        subject,
        access.memberID,
        access.operator,
        access.clerkGrant,
      ],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    return row as {
      id: string;
      name: string;
      accountID: string;
      domain: string;
      localPart: string;
      identityID: string;
      principalID: string;
      grantRevision: string;
    };
  }
  async function authorizeMailbox(
    db: Connection,
    actor: MailActor,
    customerID: string,
    mailboxID: string,
  ) {
    return mailboxAccess(
      db,
      await authorizeMail(db, actor, customerID),
      actor.subject,
      customerID,
      mailboxID,
    );
  }
  /** Only an admitted job may use this: logout does not retract committed intent, but revoked grants/bans do. */
  async function authorizeQueuedMailbox(
    db: Connection,
    context: {
      subject: string;
      customerID: string;
      mailboxID: string;
      grantRevision: string;
    },
  ) {
    const clerk = await clerkMailAccess(db, context.subject, context.customerID);
    if (clerk === null) mailUnavailable();
    const validate = clerk
      ? (user: LocalIdentity | null) => {
          if (!user || user.banned) mailUnavailable();
          return user;
        }
      : validUser;
    const user = validate(
      await identity.subject(db, context.subject, { lock: true }),
    );
    if (user.subject !== context.subject) mailUnavailable();
    const mailbox = await mailboxAccess(
      db,
      await subjectAccess(db, user, context.customerID, false, true),
      context.subject,
      context.customerID,
      context.mailboxID,
    );
    if (String(mailbox.grantRevision) !== context.grantRevision)
      mailUnavailable();
    return mailbox;
  }

  return { authorizeMail, authorizeMailbox, authorizeQueuedMailbox };
}
