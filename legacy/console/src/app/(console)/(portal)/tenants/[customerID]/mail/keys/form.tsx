"use client";
import {msgid} from "@webdock/i18n";
import { useI18n } from '@webdock/i18n/react';

import { useActionState, useState } from "react";
import { keyAction } from "./actions";
export function KeyForm({
  customer,
  operator = false,
  consumerKey,
}: {
  customer: string;
  operator?: boolean;
  consumerKey?: string;
}) {
  const i18n = useI18n();

  const [state, submit, pending] = useActionState(keyAction, {});
  const [hidden, setHidden] = useState(false),
    [copied, setCopied] = useState("");
  return (
    <form
      action={async (form) => {
        setHidden(false);
        setCopied("");
        await submit(form);
      }}
      className="access-form"
      aria-busy={pending}
    >
      <input type="hidden" name="customer" value={customer} />
      <input
        type="hidden"
        name="action"
        value={consumerKey ? "revoke" : "create"}
      />
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
        {consumerKey ? (
          <>
            <input type="hidden" name="consumerKey" value={consumerKey} />
            <label className="check">
              <input type="checkbox" required name="confirm" value="yes" />{i18n.t("Revoke this credential. Applications using it will immediately lose access.")}</label>
          </>
        ) : (
          <>
            <label className="field">{i18n.t("Label")}<input
                name="label"
                required
                maxLength={100}
                placeholder={i18n.t("Production website")}
                autoComplete="off"
              />
            </label>
            <label className="check">
              <input
                type="checkbox"
                name="permissions"
                value="SEND_SMTP"
                defaultChecked
              />{i18n.t("SMTP sending")}</label>
            <label className="check">
              <input
                type="checkbox"
                name="permissions"
                value="SEND_API"
                defaultChecked
              />{i18n.t("API sending")}</label>
            {operator && (
              <label className="check">
                <input type="checkbox" name="permissions" value="APIS" />{i18n.t("Provider administration (operator only)")}</label>
            )}
            <label className="field">{i18n.t("Allowed IP addresses (optional)")}<textarea
                name="ips"
                rows={3}
                maxLength={4600}
                placeholder={i18n.t("203.0.113.10, 2001:db8::10")}
              />
              <span className="muted">{i18n.t("Individual IPv4 or IPv6 addresses, separated by commas or spaces. Blank allows any IP address.")}</span>
            </label>
          </>
        )}
        <button type="submit" className="button secondary">
          {pending
            ? i18n.t("Working…")
            : consumerKey
              ? i18n.t("Revoke credential")
              : i18n.t("Create credential")}
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
      {state.created && !hidden && !pending && (
        <section className="notice">
          <h3>{i18n.t("Save your new credential")}</h3>
          <p>{i18n.t("This secret is shown only now. Store it securely before leaving or dismissing this panel.")}</p>
          <dl>
            <dt>{i18n.t("Consumer key")}</dt>
            <dd style={{ overflowWrap: "anywhere" }}>
              <code>{state.created.consumerKey}</code>
            </dd>
            <dt>{i18n.t("Consumer secret")}</dt>
            <dd style={{ overflowWrap: "anywhere" }}>
              <code>{state.created.consumerSecret}</code>
            </dd>
          </dl>
          <button
            className="button secondary"
            type="button"
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(
                  i18n.t("Consumer key: {key}\nConsumer secret: {secret}",{key:state.created!.consumerKey,secret:state.created!.consumerSecret}),
                );
                setCopied(msgid("Copied."));
              } catch {
                setCopied(msgid("Copy failed. Select and copy the values above."));
              }
            }}
          >{i18n.t("Copy credentials")}</button>{" "}
          <button
            className="button secondary"
            type="button"
            onClick={() => setHidden(true)}
          >{i18n.t("Dismiss secret")}</button>
          <span role="status">{i18n.t(copied)}</span>
        </section>
      )}
    </form>
  );
}
