"use client";
import { useI18n } from "@webdock/i18n/react";
import { useActionState, type ReactNode } from "react";
import { accessAction } from "./actions";
export function AccessForm({
  action,
  label,
  children,
  confirm,
}: {
  action: string;
  label: string;
  children?: ReactNode;
  confirm?: string;
}) {
 const { t, error: translateError } = useI18n();
  const [state, submit, pending] = useActionState(accessAction, {});
  return (
    <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="action" value={action} />
      {children}
      {confirm && (<label className="check">
          <input type="checkbox" required /> {t(confirm)}
        </label>)}
      {state.error && (<p className="notice error" role="alert">
          {translateError(state.error)}
        </p>)}
      {state.message && (<p className="notice" role="status">
          {t(state.message)}
        </p>)}
      <button className="button secondary" disabled={pending}>
        {pending ? t("Saving…") : t(label)}
      </button>
    </form>
  );
}
export function RoleSelect({ value = "editor" }: { value?: string }) {
 const { t } = useI18n();
  return (
    <label className="field">
      {t("Website role")}<select name="role" defaultValue={value}>
        <option value="reader">{t("Reader — view content")}</option>
        <option value="editor">{t("Editor — edit and publish content")}</option>
        <option value="admin">{t("Administrator — manage, delete and restore content")}</option>
      </select>
    </label>
  );
}
