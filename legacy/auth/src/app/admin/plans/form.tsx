"use client";
import { useI18n } from "@webdock/i18n/react";
import { useActionState, type ReactNode } from "react";
import { planAction } from "./actions";
export function PlanForm({ action, label, children }: { action: string; label: string; children: ReactNode }) {
 const { t, error: translateError } = useI18n();
  const [state, submit, pending] = useActionState(planAction, {});
  return <form action={submit} className="access-form" aria-busy={pending}>
    <input type="hidden" name="action" value={action} />{children}
    {state.error && <p className="notice error" role="alert">{translateError(state.error)}</p>}
    {state.message && <p className="notice" role="status">{t(state.message)}</p>}
    <button className="button secondary" disabled={pending}>{pending ? t("Saving…") : t(label)}</button>
  </form>;
}
