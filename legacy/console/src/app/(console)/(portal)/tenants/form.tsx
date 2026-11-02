"use client";
import { useI18n } from '@webdock/i18n/react';

import { useActionState, type ReactNode } from "react";
import { tenantAction } from "./actions";
export function TenantForm({
  customer,
  action,
  label,
  children,
  confirm,
}: {
  customer: string;
  action: string;
  label: string;
  children?: ReactNode;
  confirm?: string;
}) {
  const i18n = useI18n();

  const [state, submit, pending] = useActionState(tenantAction, {});
  return (
    <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="customer" value={customer} />
      <input type="hidden" name="action" value={action} />
      {children}
      {confirm && (
        <label className="check">
          <input type="checkbox" required />
          {confirm}
        </label>
      )}
      {state.error && (
        <p className="notice error" role="alert">
          {i18n.error(state.error)}
        </p>
      )}
      {state.message && (
        <p className="notice" role="status">
          {i18n.error(state.message, "Changes saved.")}
        </p>
      )}
      <button className="button secondary" disabled={pending}>
        {pending ? i18n.t("Saving…") : i18n.t(label)}
      </button>
    </form>
  );
}
export function TenantRole({ value = "member", allowOwner = false, memberOnly = false }: { value?: string; allowOwner?: boolean; memberOnly?: boolean }) {
  const i18n = useI18n();

  return (
    <label className="field">{i18n.t("Workspace role")}<select name="role" defaultValue={value === "owner" && !allowOwner ? "admin" : value}>
        <option value="member">{i18n.t("Member — view customer profile")}</option>
        {!memberOnly&&<option value="admin">{i18n.t("Administrator — edit profile and invite people")}</option>}
        {allowOwner&&<option value="owner">{i18n.t("Owner — manage administrators and owners")}</option>}
      </select>
    </label>
  );
}
