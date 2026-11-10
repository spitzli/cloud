import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { MailError } from "./product-access";
export function createMailAnalytics(
  dependencies: Pick<MailProductStore, "database" | "assertMailAccess">,
) {
  const { database, assertMailAccess } = dependencies;
  async function getMailAnalytics(
    actor: MailActor,
    input: { days: 7 | 30 | 90; domainID?: string; includeSimulated?: boolean },
  ) {
    await assertMailAccess(actor, "read");
    if (
      ![7, 30, 90].includes(input.days) ||
      typeof (input.includeSimulated ?? false) !== "boolean"
    )
      throw new MailError(422, "validation_error", "Invalid analytics filters");
    if (input.domainID) parseID(input.domainID);
    const until = new Date();
    until.setUTCHours(0, 0, 0, 0);
    until.setUTCDate(until.getUTCDate() + 1);
    const from = new Date(until.getTime() - input.days * 86400000);
    const rows = (
      await database.query(
        `SELECT to_char(coalesce(m.accepted_at,m.terminal_at,m.created_at) AT TIME ZONE 'UTC','YYYY-MM-DD') AS date,
 count(DISTINCT m.id) FILTER(WHERE m.accepted_at IS NOT NULL)::int AS "messagesAccepted",
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL)::int AS "recipientsAccepted",
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.delivery_status='delivered')::int AS delivered,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.delivery_status='bounced')::int AS bounced,
 count(r.message_id) FILTER(WHERE r.suppression_reason IS NOT NULL)::int AS suppressed,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.delivery_status='delivered' AND r.first_open_at IS NOT NULL)::int AS opened,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.delivery_status='delivered' AND r.first_click_at IS NOT NULL)::int AS clicked,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.complained_at IS NOT NULL)::int AS complained,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.unsubscribed_at IS NOT NULL)::int AS unsubscribed,
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.first_open_at IS NOT NULL)::int AS "rawOpened",
 count(r.message_id) FILTER(WHERE m.accepted_at IS NOT NULL AND r.suppression_reason IS NULL AND r.first_click_at IS NOT NULL)::int AS "rawClicked"
 FROM webdock_auth.mail_message m LEFT JOIN webdock_auth.mail_recipient r ON r.message_id=m.id AND r.customer_id=m.customer_id AND r.connection_id=m.connection_id
 WHERE m.customer_id=$1 AND m.connection_id=$2 AND ($3::text IS NULL OR m.domain_id=$3) AND ($4 OR NOT m.simulated)
 AND coalesce(m.accepted_at,m.terminal_at,m.created_at)>=$5 AND coalesce(m.accepted_at,m.terminal_at,m.created_at)<$6 GROUP BY 1`,
        [
          actor.customerID,
          actor.connectionID,
          input.domainID ?? null,
          !!input.includeSimulated,
          from,
          until,
        ],
      )
    ).rows;
    const totals = {
      messagesAccepted: 0,
      recipientsAccepted: 0,
      delivered: 0,
      bounced: 0,
      suppressed: 0,
      opened: 0,
      clicked: 0,
      complained: 0,
      unsubscribed: 0,
      rawOpened: 0,
      rawClicked: 0,
    };
    for (const row of rows)
      for (const key of Object.keys(totals) as (keyof typeof totals)[])
        totals[key] += row[key];
    const days = Array.from({ length: input.days }, (_, i) => {
      const date = new Date(from.getTime() + i * 86400000)
          .toISOString()
          .slice(0, 10),
        row = rows.find((r) => r.date === date);
      return {
        date,
        accepted: row?.recipientsAccepted ?? 0,
        delivered: row?.delivered ?? 0,
        bounced: row?.bounced ?? 0,
      };
    });
    const { recipientsAccepted, delivered, bounced, opened, clicked } = totals;
    return {
      ...totals,
      deliveryRate: recipientsAccepted ? delivered / recipientsAccepted : null,
      bounceRate: recipientsAccepted ? bounced / recipientsAccepted : null,
      openRate: delivered ? opened / delivered : null,
      clickRate: delivered ? clicked / delivered : null,
      days,
      asOf: new Date().toISOString(),
      simulatedIncluded: !!input.includeSimulated,
    };
  }

  return { getMailAnalytics };
}
