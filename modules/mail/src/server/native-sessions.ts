import { createHash, randomBytes } from "node:crypto";
import { openSecret } from "./secrets";
import { type Connection } from "./native-ports";
import { mailUnavailable, type MailActor } from "./native-access";
import type { createNativeMailAuthorization } from "./native-access";
import type { NativeMailPorts } from "./native-ports";
const hash = (token: string) =>
  createHash("sha256").update(token).digest("hex");

const token = () => randomBytes(32).toString("base64url");

type BoundSession = {
  id: string;
  customer_id: string;
  mailbox_id: string;
  subject: string;
  session_id: string;
  grant_revision: string;
  expires_at: Date;
};
export interface NativeSessionsDependencies {
  resolveSessionReference: NativeMailPorts["resolveSessionReference"];
  withAuthenticatedDatabase: NativeMailPorts["withAuthenticatedDatabase"];
  authorizeQueuedMailbox: ReturnType<
    typeof createNativeMailAuthorization
  >["authorizeQueuedMailbox"];
  authorizeMailbox: ReturnType<
    typeof createNativeMailAuthorization
  >["authorizeMailbox"];
  env: NativeMailPorts["env"];
  transaction: NativeMailPorts["transaction"];
}
export function createNativeSessions(dependencies: NativeSessionsDependencies) {
  const {
    resolveSessionReference,
    withAuthenticatedDatabase,
    authorizeQueuedMailbox,
    authorizeMailbox,
    env,
    transaction,
  } = dependencies;
  function enabled() {
    if (env.SPITZLI_NATIVE_MAILBOXES_ENABLED !== "true") mailUnavailable();
  }

  function validToken(value: string) {
    if (!/^[A-Za-z0-9_-]{43}$/.test(value)) mailUnavailable();
  }

  function gatewayOrigin() {
    const origin = new URL(
      env.MAIL_GATEWAY_ORIGIN || "https://unconfigured.invalid",
    );
    if (
      !env.MAIL_GATEWAY_ORIGIN ||
      origin.protocol !== "https:" ||
      origin.username ||
      origin.password ||
      origin.pathname !== "/" ||
      origin.search ||
      origin.hash
    )
      mailUnavailable();
    return origin.origin;
  }

  async function launchWebmail(
    db: Connection,
    actor: MailActor,
    customerID: string,
    mailboxID: string,
  ) {
    enabled();
    const mailbox = await authorizeMailbox(db, actor, customerID, mailboxID);
    const origin = gatewayOrigin(),
      code = token();
    // Bound metadata growth without a separate cleanup service.
    await db.query(
      "DELETE FROM webdock_mail.webmail_launch WHERE expires_at<now()",
    );
    await db.query(
      "DELETE FROM webdock_mail.webmail_session WHERE expires_at<now()",
    );
    const count = (
      await db.query(
        "SELECT count(*)::int AS count FROM webdock_mail.webmail_launch WHERE session_id=$1 AND consumed_at IS NULL",
        [actor.sessionID],
      )
    ).rows[0].count;
    if (count >= 20) mailUnavailable();
    const row = (
      await db.query(
        `INSERT INTO webdock_mail.webmail_launch(token_hash,customer_id,mailbox_id,subject,session_id,grant_revision,expires_at)
    VALUES($1,$2,$3,$4,$5,$6,now()+interval '60 seconds') RETURNING expires_at`,
        [
          hash(code),
          customerID,
          mailboxID,
          actor.subject,
          actor.sessionID,
          mailbox.grantRevision,
        ],
      )
    ).rows[0];
    return {
      code,
      gatewayOrigin: origin,
      mailboxID,
      customerID,
      expiresAt: new Date(row.expires_at).toISOString(),
    };
  }

  async function authorizeBound(db: Connection, row: BoundSession) {
    const mailbox = await authorizeMailbox(
      db,
      { subject: row.subject, sessionID: row.session_id },
      row.customer_id,
      row.mailbox_id,
    );
    if (String(mailbox.grantRevision) !== String(row.grant_revision))
      mailUnavailable();
    return mailbox;
  }

  async function exchangeWebmail(db: Connection, code: string) {
    enabled();
    validToken(code);
    const row = (
      await db.query(
        `UPDATE webdock_mail.webmail_launch SET consumed_at=now()
    WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING *`,
        [hash(code)],
      )
    ).rows[0] as BoundSession | undefined;
    if (!row) mailUnavailable();
    await authorizeBound(db, row);
    const value = token();
    const session = (
      await db.query(
        `INSERT INTO webdock_mail.webmail_session(token_hash,customer_id,mailbox_id,subject,session_id,grant_revision,expires_at)
    VALUES($1,$2,$3,$4,$5,$6,now()+interval '15 minutes') RETURNING id,expires_at`,
        [
          hash(value),
          row.customer_id,
          row.mailbox_id,
          row.subject,
          row.session_id,
          row.grant_revision,
        ],
      )
    ).rows[0];
    return {
      token: value,
      scopeID: session.id as string,
      customerID: row.customer_id,
      mailboxID: row.mailbox_id,
      expiresAt: new Date(session.expires_at).toISOString(),
    };
  }

  /** Gateway-only: credentials are never returned through a browser/Studio operation. */
  async function inspectWebmail(db: Connection, value: string) {
    enabled();
    validToken(value);
    const row = (
      await db.query(
        "SELECT * FROM webdock_mail.webmail_session WHERE token_hash=$1 AND expires_at>now() FOR SHARE",
        [hash(value)],
      )
    ).rows[0] as BoundSession | undefined;
    if (!row) mailUnavailable();
    return inspectRow(db, row);
  }

  async function inspectRow(db: Connection, row: BoundSession) {
    const mailbox = await authorizeBound(db, row);
    return {
      ...(await mailboxConnection(db, row.customer_id, mailbox)),
      scopeID: row.id,
      subject: row.subject,
      grantRevision: String(mailbox.grantRevision),
      expiresAt: new Date(row.expires_at).toISOString(),
    };
  }

  /** Background processing is bound to a previously admitted mailbox/grant, not an arbitrary subject. */
  async function inspectQueuedWebmail(
    db: Connection,
    context: {
      customerID: string;
      subject: string;
      mailboxID: string;
      grantRevision: string;
    },
  ) {
    enabled();
    const mailbox = await authorizeQueuedMailbox(db, context);
    return {
      ...(await mailboxConnection(db, context.customerID, mailbox)),
      subject: context.subject,
    };
  }

  async function mailboxConnection(
    db: Connection,
    customerID: string,
    mailbox: Awaited<ReturnType<typeof authorizeMailbox>>,
  ) {
    const service = (
      await db.query(
        "SELECT host_id FROM webdock_mail.service WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    if (
      !env.MAIL_MANAGED_CLUSTER_ID ||
      service.host_id !== env.MAIL_MANAGED_CLUSTER_ID
    )
      mailUnavailable();
    const identity = (
      await db.query(
        "SELECT encrypted_credentials FROM webdock_mail.identity WHERE id=$1 AND customer_id=$2",
        [mailbox.identityID, customerID],
      )
    ).rows[0];
    if (!identity?.encrypted_credentials) mailUnavailable();
    const secret = openSecret<{ username: string; password: string }>(
      identity.encrypted_credentials,
      `identity:${customerID}:${mailbox.identityID}`,
      env.MAIL_ENCRYPTION_KEY || "",
    );
    if (
      typeof secret.username !== "string" ||
      !secret.username ||
      /[:\r\n\0]/.test(secret.username) ||
      typeof secret.password !== "string" ||
      !secret.password ||
      /[\r\n\0]/.test(secret.password)
    )
      mailUnavailable();
    return {
      customerID,
      mailboxID: mailbox.id,
      accountID: mailbox.accountID,
      senderName: mailbox.name,
      senderAddress: `${mailbox.localPart}@${mailbox.domain}`,
      runtimeOrigin: `http://app-${customerID}.wd-mail-${customerID}.svc.cluster.local:8080`,
      authorization: `Basic ${Buffer.from(`${secret.username}:${secret.password}`).toString("base64")}`,
    };
  }

  async function revokeWebmail(db: Connection, value: string) {
    validToken(value);
    await db.query(
      "DELETE FROM webdock_mail.webmail_session WHERE token_hash=$1",
      [hash(value)],
    );
    return { ok: true };
  }

  async function launchTenantWebmail(
    headers: Headers,
    customerID: string,
    mailboxID: string,
  ) {
    return withAuthenticatedDatabase(headers, async () => {
      const session = await resolveSessionReference(headers);
      if (!session) mailUnavailable();
      return transaction((db) =>
        launchWebmail(
          db,
          { subject: session.subject, sessionID: session.sessionID },
          customerID,
          mailboxID,
        ),
      );
    });
  }
  return {
    launchWebmail,
    exchangeWebmail,
    inspectWebmail,
    inspectQueuedWebmail,
    revokeWebmail,
    launchTenantWebmail,
  };
}
