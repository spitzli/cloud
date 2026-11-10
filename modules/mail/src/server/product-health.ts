import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
type Dependencies = {
  database: MailProductStore["database"];
  assertMailAccess: MailProductStore["assertMailAccess"];
};

export function createMailHealth(dependencies: Dependencies) {
  const { database, assertMailAccess } = dependencies;
  async function getMailHealth(actor: MailActor) {
    await assertMailAccess(actor, "read");
    const [jobs, queue, callbacks, flags, outbox, credentials] =
      await Promise.all([
        database.query(
          `SELECT DISTINCT ON(name) name,status,started_at,finished_at,max(finished_at) FILTER(WHERE status='succeeded') OVER(PARTITION BY name) AS last_success FROM webdock_auth.mail_job_run ORDER BY name,started_at DESC`,
        ),
        database.query(
          `SELECT count(*) FILTER(WHERE dispatch_status='outcome_unknown')::int AS unknown,
   count(*) FILTER(WHERE dispatch_status='scheduled' AND blocked_reason IS NOT NULL)::int AS blocked,
   min(scheduled_at) FILTER(WHERE dispatch_status='scheduled' AND blocked_reason IS NULL) AS oldest_due,
   min(coalesce(handoff_started_at,created_at)) FILTER(WHERE dispatch_status='outcome_unknown') AS oldest_unknown FROM webdock_auth.mail_message WHERE customer_id=$1 AND connection_id=$2`,
          [actor.customerID, actor.connectionID],
        ),
        database.query(
          `SELECT EXISTS(SELECT 1 FROM webdock_auth.mail_callback_source_connection b JOIN webdock_auth.mail_callback_source s ON s.id=b.source_id WHERE b.connection_id=$1 AND s.enabled AND s.verified_at IS NOT NULL) AS configured,
   (SELECT max(created_at) FROM webdock_auth.mail_event WHERE connection_id=$1 AND customer_id=$2 AND source='provider') AS last_event`,
          [actor.connectionID, actor.customerID],
        ),
        database.query(
          "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
        ),
        database.query(
          `SELECT count(*)::int AS pending,min(next_at) AS oldest_due FROM webdock_auth.mail_webhook_delivery WHERE customer_id=$1 AND connection_id=$2 AND status IN ('pending','delivering')`,
          [actor.customerID, actor.connectionID],
        ),
        database.query(
          "SELECT EXISTS(SELECT 1 FROM webdock_auth.mail_connection_secret WHERE customer_id=$1 AND connection_id=$2 AND active) AS configured",
          [actor.customerID, actor.connectionID],
        ),
      ]);
    const operator =
      actor.principal.kind === "session" &&
      (
        await database.query(
          'SELECT 1 FROM webdock_auth."user" WHERE id=$1 AND role=\'operator\' AND "twoFactorEnabled"',
          [actor.principal.userID],
        )
      ).rowCount;
    const inbox = operator
      ? (
          await database.query(
            `SELECT count(*) FILTER(WHERE i.status IN ('pending','processing'))::int AS pending,count(*) FILTER(WHERE i.status='quarantined')::int AS quarantined,min(i.created_at) FILTER(WHERE i.status IN ('pending','processing')) AS oldest_due FROM webdock_auth.mail_callback_inbox i JOIN webdock_auth.mail_callback_source_connection b ON b.source_id=i.source_id WHERE b.connection_id=$1`,
            [actor.connectionID],
          )
        ).rows[0]
      : null;
    const settings = flags.rows[0]?.settings ?? {},
      now = Date.now(),
      state = queue.rows[0];
    const minutes = (value: Date | null | undefined) =>
      value ? (now - new Date(value).getTime()) / 60000 : null;
    return {
      gatewayEnabled: settings.mailGatewayEnabled === true,
      dispatchEnabled: settings.mailDispatchEnabled === true,
      webhooksEnabled: settings.mailWebhooksEnabled === true,
      jobs: jobs.rows,
      queue: state,
      callbacks: callbacks.rows[0],
      outbox: outbox.rows[0],
      inbox,
      credentials: credentials.rows[0],
      alerts: {
        inboxLate: (minutes(inbox?.oldest_due) ?? 0) > 2,
        tickLate:
          settings.mailGatewayEnabled === true &&
          settings.mailDispatchEnabled === true &&
          (minutes(
            jobs.rows.find((r) => r.name === "dispatch")?.last_success,
          ) ?? Infinity) > 3,
        dueLate: (minutes(state.oldest_due) ?? 0) > 5,
        unknownLate: (minutes(state.oldest_unknown) ?? 0) > 5,
      },
    };
  }
  return { getMailHealth };
}
