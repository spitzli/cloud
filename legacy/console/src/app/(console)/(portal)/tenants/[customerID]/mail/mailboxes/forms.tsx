"use client";
import { useActionState, type ReactNode } from "react";
import { useI18n } from "@webdock/i18n/react";
import { saveMailbox } from "./actions";
export function MailboxForm({ customerID, action, label, children }: { customerID: string; action: string; label: string; children?: ReactNode }) {
  const { t, error } = useI18n();
  const [result, submit, pending] = useActionState(saveMailbox, {});
  return <form action={submit} className="access-form mailbox-form" aria-busy={pending}>
    <input type="hidden" name="customer" value={customerID} />
    <input type="hidden" name="action" value={action} />
    <fieldset disabled={pending}>{children}<button type="submit" className="button secondary">{pending ? t("Saving…") : label}</button></fieldset>
    {result.error && <p className="notice error" role="alert">{error(result.error)}</p>}
    {result.message && <p className="notice" role="status">{error(result.message, "Changes saved.")}</p>}
  </form>;
}
