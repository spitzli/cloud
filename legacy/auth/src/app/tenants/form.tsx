"use client";
import { useI18n } from "@webdock/i18n/react";
import { useActionState, type ReactNode } from "react";
import { tenantAction } from "./actions";
export function TenantForm({ customer, action, label, children, confirm }: { customer: string; action: string; label: string; children?: ReactNode; confirm?: string }) {
 const { t, error: translateError } = useI18n();
  const [state, submit, pending] = useActionState(tenantAction, {});
  return <form action={submit} className="access-form" aria-busy={pending}>
    <input type="hidden" name="customer" value={customer} />
    <input type="hidden" name="action" value={action} />
    {children}
    {confirm && <label className="check"><input type="checkbox" required />{t(confirm)}</label>}
    {state.error && <p className="notice error" role="alert">{translateError(state.error)}</p>}
    {state.message && <p className="notice" role="status">{t(state.message)}</p>}
    <button className="button secondary" disabled={pending}>{pending ? t("Saving…") : t(label)}</button>
  </form>;
}
export function TenantRole({ value = "member" }: { value?: string }) {
 const { t } = useI18n();
  return <label className="field">{t("Tenant role")}<select name="role" defaultValue={value === "owner" ? "admin" : value}>
    <option value="member">{t("Member — view customer profile")}</option><option value="admin">{t("Administrator — edit profile and invite people")}</option>
  </select></label>;
}
