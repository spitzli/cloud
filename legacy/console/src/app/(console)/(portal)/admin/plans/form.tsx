"use client";
import { useI18n } from '@webdock/i18n/react';

import { useActionState, type ReactNode } from "react";
import { planAction } from "./actions";
export function PlanForm({
  action,
  label,
  children,
}: {
  action: string;
  label: string;
  children: ReactNode;
}) {
  const i18n = useI18n();

  const [state, submit, pending] = useActionState(planAction, {});
  return (
    <form action={submit} className="access-form" aria-busy={pending}>
      <input type="hidden" name="action" value={action} />
      {children}
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
