"use client";
import { useI18n } from '@webdock/i18n/react';


import { useActionState, type ReactNode } from "react";
import { mailAction } from "./actions";

export function MailForm({
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

  const [state, submit, pending] = useActionState(mailAction, {});
  return (
    <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="customer" value={customer} />
      <input type="hidden" name="action" value={action} />
      <fieldset
        disabled={pending}
        style={{
          border: 0,
          padding: 0,
          margin: 0,
          minWidth: 0,
          display: "grid",
          gap: 14,
        }}
      >
        {children}
        {confirm && (
          <label className="check">
            <input name="confirm" type="checkbox" value="yes" required />
            {confirm}
          </label>
        )}
        <button className="button secondary" type="submit">
          {pending ? i18n.t("Working…") : i18n.t(label)}
        </button>
      </fieldset>
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
    </form>
  );
}
