import Link from "next/link";
import { msgid } from "@spitzli/i18n";
import type { MailPageContext, MailRow } from "./types";
export type { MailPageContext } from "./types";
export function mailHref(
  context: MailPageContext,
  path: string,
  extra: Record<string, string> = {},
) {
  const query = new URLSearchParams({
    ...(context.connection ? { connection: context.connection.id } : {}),
    ...extra,
  });
  return context.base + path + (query.size ? "?" + query.toString() : "");
}
export function MailPageHeading({
  context,
  title,
  embedded = false,
}: {
  context: MailPageContext;
  title: string;
  embedded?: boolean;
}) {
  const { i18n } = context;
  const Heading = embedded ? "h2" : "h1";
  return (
    <header className="account-heading">
      <div>
        <Heading>{title}</Heading>
        <p className="muted">{context.tenantName}</p>
      </div>
      {context.connections.length > 0 && (
        <form method="get" className="access-form">
          <label className="field">
            {i18n.t("Sending connection")}
            <select name="connection" defaultValue={context.connection?.id}>
              {context.connections.map((c) => (
                <option value={c.id} key={c.id}>
                  {c.kind === "byok"
                    ? i18n.t("Own provider")
                    : i18n.t("Managed Mail")}{" "}
                  · {c.region.toUpperCase()} ·{" "}
                  {i18n.t(stateLabels[c.state] ?? c.state)}
                </option>
              ))}
            </select>
          </label>
          <button className="button secondary">
            {i18n.t("Select connection")}
          </button>
        </form>
      )}
    </header>
  );
}
export function MailNoConnection({ context }: { context: MailPageContext }) {
  return (
    <section className="auth-panel">
      <p>
        {context.i18n.t("Connect a sending account to view Mail activity.")}
      </p>
      {context.canManage ? (
        <Link href={context.base + "/settings"}>
          {context.i18n.t("Connect a sending account")}
        </Link>
      ) : (
        <p>
          {context.i18n.t(
            "Ask an owner or admin to set up sending for this tenant.",
          )}
        </p>
      )}
    </section>
  );
}
export const stateLabels: Record<string, string> = {
  ownership_verified: msgid("Ownership verified"),
  sending_access: msgid("Sending only"),
  full_access: msgid("Full Mail API access"),
  manual: msgid("Manual"),
  complained: msgid("Complaint"),
  unsubscribed: msgid("Unsubscribed"),
  scheduled: msgid("Scheduled"),
  dispatching: msgid("Sending"),
  accepted: msgid("Accepted"),
  failed: msgid("Failed"),
  outcome_unknown: msgid("Outcome unknown"),
  canceled: msgid("Canceled"),
  suppressed: msgid("Suppressed"),
  delivered: msgid("Delivered"),
  bounced: msgid("Bounced"),
  pending: msgid("Pending"),
  ready: msgid("Ready"),
  paused: msgid("Paused"),
  needs_review: msgid("Needs review"),
  disconnected: msgid("Disconnected"),
  "email.sent": msgid("Accepted"),
  "email.delivered": msgid("Delivered"),
  "email.bounced": msgid("Bounced"),
  "email.complained": msgid("Complaint"),
  "email.unsubscribed": msgid("Unsubscribed"),
  "email.clicked": msgid("Clicked"),
  "email.opened": msgid("Opened"),
  "email.delivery_delayed": msgid("Delivery delayed"),
  "email.scheduled": msgid("Scheduled"),
  "email.canceled": msgid("Canceled"),
  "email.failed": msgid("Failed"),
  "email.suppressed": msgid("Suppressed"),
};
export function MailRows({
  context,
  rows,
  columns,
  detailPath,
  actions,
  emptyText,
}: {
  context: MailPageContext;
  emptyText?: string;
  rows: MailRow[];
  columns: { key: string; label: string }[];
  detailPath?: string;
  actions?: (row: MailRow) => React.ReactNode;
}) {
  if (!rows.length)
    return (
      <p className="notice">{emptyText ?? context.i18n.t("No entries yet.")}</p>
    );
  const display = (value: unknown, key: string) => {
    if (value === null || value === undefined) return "—";
    if (key === "status" && value === 0) return context.i18n.t("Unfinished");
    if (
      typeof value === "string" &&
      [
        "status",
        "dispatch_status",
        "delivery_status",
        "permission",
        "reason",
        "state",
      ].includes(key) &&
      stateLabels[value]
    )
      return context.i18n.t(stateLabels[value]);
    if (
      typeof value === "string" &&
      key.endsWith("_at") &&
      Number.isFinite(Date.parse(value))
    )
      return context.i18n.date(value, {
        dateStyle: "short",
        timeStyle: "short",
      });
    return Array.isArray(value)
      ? value.join(", ")
      : typeof value === "boolean"
        ? context.i18n.t(value ? "Yes" : "No")
        : String(value);
  };
  return (
    <div style={{ overflowX: "auto" }}>
      <table
        className="data-table"
        style={{ width: "100%", textAlign: "left" }}
      >
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column.key} scope="col">
                {column.label}
              </th>
            ))}
            {actions && <th scope="col">{context.i18n.t("Actions")}</th>}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={row.id ?? String(row.email ?? index)}>
              {columns.map((column, i) => (
                <td
                  key={column.key}
                  style={{ padding: "10px 8px", overflowWrap: "anywhere" }}
                >
                  {i === 0 && detailPath ? (
                    <Link href={mailHref(context, detailPath + "/" + row.id)}>
                      {display(row[column.key], column.key)}
                    </Link>
                  ) : (
                    display(row[column.key], column.key)
                  )}
                </td>
              ))}
              {actions && <td>{actions(row)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
