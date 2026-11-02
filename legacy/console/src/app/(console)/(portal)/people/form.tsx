"use client";
import { useI18n } from '@webdock/i18n/react';

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
  const i18n = useI18n();

  const [state, submit, pending] = useActionState(accessAction, {});
  return (
    <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="action" value={action} />
      {children}
      {confirm && (
        <label className="check">
          <input type="checkbox" required /> {confirm}
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
export function RoleSelect({ value = "editor" }: { value?: string }) {
  const i18n = useI18n();

  return (
    <label className="field">{i18n.t("Website role")}<select name="role" defaultValue={value}>
        <option value="reader">{i18n.t("Reader — view content")}</option>
        <option value="editor">{i18n.t("Editor — edit and publish content")}</option>
        <option value="admin">{i18n.t("Administrator — manage, delete and restore content")}</option>
      </select>
    </label>
  );
}
