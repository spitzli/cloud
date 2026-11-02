"use client";
import Link from "next/link";
import { useActionState } from "react";
import { useI18n } from "@webdock/i18n/react";
import type { NativeMailView } from "@/lib/studio-contracts";
import { mailServiceAction } from "./service-actions";

export function MailServicePanel({ data }: { data: NonNullable<NativeMailView> }) {
  const i18n = useI18n();
  const [result, submit, pending] = useActionState(mailServiceAction, {});
  const { service } = data;
  const labels = { disabled: i18n.t("Not activated"), pending: i18n.t("Change queued"), provisioning: i18n.t("Applying change"), ready: i18n.t("Ready"), suspended: i18n.t("Suspended — data retained"), needs_review: i18n.t("Operator review required") };
  const allowed = service.enabled ? data.canSuspend : data.canActivate;
  return <section className="auth-panel">
    <h2>{i18n.t("Incoming Mail")}</h2>
    <p role="status">{labels[service.state]}</p>
    <p className="muted">{i18n.t("Activate incoming Mail to manage mailboxes and use Webmail. Sending-only does not require activation. Suspending retains mailbox data.")}</p>
    {service.state === "needs_review" && <p className="notice">{i18n.t("The previous change needs to be checked before setup can continue.")}</p>}
    {allowed && <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="customer" value={service.customerID} />
      <input type="hidden" name="revision" value={service.revision} />
      <input type="hidden" name="action" value={service.enabled ? "suspend" : "activate"} />
      <button className="button secondary" type="submit" disabled={pending}>{pending ? i18n.t("Saving…") : service.enabled ? i18n.t("Suspend incoming Mail") : i18n.t("Activate incoming Mail")}</button>
    </form>}
    {data.canReconcile && <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="customer" value={service.customerID} />
      <input type="hidden" name="revision" value={service.revision} />
      <input type="hidden" name="action" value="reconcile" />
      <button className="button" type="submit" disabled={pending}>{i18n.t("Retry Mail operation")}</button>
    </form>}
    {data.sendingAvailable && <p><Link href={`/tenants/${service.customerID}/mail/sending`}>{i18n.t("Webdock Mail sending")}</Link></p>}
    {data.mailboxesAvailable && <p><Link href={`/tenants/${service.customerID}/mail/mailboxes`}>{i18n.t("Manage mailboxes")}</Link></p>}
    {result.error && <p className="notice error" role="alert">{i18n.error(result.error)}</p>}
    {result.message && <p className="notice" role="status">{i18n.error(result.message, "Changes saved.")}</p>}
  </section>;
}
