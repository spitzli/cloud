"use client";
import { useI18n } from '@webdock/i18n/react';

import { useActionState } from "react";
import Link from "next/link";
import { acceptOfferAction } from "./actions";
export function OfferForm({ token }: { token: string }) {
  const i18n = useI18n();

  const [state, submit, pending] = useActionState(
    acceptOfferAction.bind(null, token),
    {},
  );
  return state.customerID ? (
    <div className="notice" role="status">
      <p>{i18n.error(state.message, "Changes saved.")}</p>
      <Link href={`/tenants/${state.customerID}/usage`}>{i18n.t("View your assigned plan")}</Link>
    </div>
  ) : (
    <form action={submit} className="access-form" aria-busy={pending}>
      <label className="check">
        <input name="confirm" type="checkbox" value="yes" required />{i18n.t("I accept this offer and its stated terms, and want to assign this plan to my tenant.")}</label>
      {state.error && (
        <p className="notice error" role="alert">
          {i18n.error(state.error)}
        </p>
      )}
      <button className="button" disabled={pending}>
        {pending ? i18n.t("Accepting…") : i18n.t("Accept offer and assign plan")}
      </button>
    </form>
  );
}
