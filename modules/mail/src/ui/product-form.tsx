"use client";
import { useActionState, useState } from "react";
import Link from "next/link";
import { useI18n } from "@spitzli/i18n/react";
import type { MailAction } from "./types";
export type MailField = {
  name: string;
  label: string;
  type?:
    | "text"
    | "email"
    | "password"
    | "url"
    | "textarea"
    | "select"
    | "checkboxes";
  required?: boolean;
  value?: string;
  options?: { value: string; label: string }[];
};
export type MailProductFormProps = {
  action: MailAction;
  customer: string;
  connection?: string;
  operation: string;
  label: string;
  fields?: MailField[];
  hidden?: Record<string, string>;
  confirm?: boolean;
  requestID?: string;
};
export function MailProductForm({
  action: mailProductAction,
  customer,
  connection = "",
  operation,
  label,
  fields = [],
  hidden = {},
  confirm = false,
  requestID,
}: MailProductFormProps) {
  const i18n = useI18n();
  const [state, action, pending] = useActionState(mailProductAction, {});
  const [copiedSecret, setCopiedSecret] = useState<string>();
  const secret = state.key ?? state.secret;
  return (
    <form
      action={action}
      className="access-form mail-product-form"
      aria-busy={pending}
    >
      <input type="hidden" name="customer" value={customer} />
      <input type="hidden" name="connection" value={connection} />
      <input type="hidden" name="operation" value={operation} />
      {requestID && (
        <input type="hidden" name="idempotencyKey" value={requestID} />
      )}
      {Object.entries(hidden).map(([name, value]) => (
        <input key={name} type="hidden" name={name} value={value} />
      ))}
      <fieldset
        disabled={
          pending ||
          state.uncertain ||
          (operation === "mailTestSend" && !!state.message)
        }
        style={{ border: 0, padding: 0, minWidth: 0, display: "grid", gap: 12 }}
      >
        {fields.map((field) =>
          field.type === "checkboxes" ? (
            <fieldset key={field.name}>
              <legend>{field.label}</legend>
              {field.options?.map((option) => (
                <label className="check" key={option.value}>
                  <input
                    type="checkbox"
                    name={field.name}
                    value={option.value}
                    defaultChecked={field.value
                      ?.split(",")
                      .includes(option.value)}
                  />
                  {option.label}
                </label>
              ))}
            </fieldset>
          ) : (
            <label className="field" key={field.name}>
              {field.label}
              {field.type === "textarea" ? (
                <textarea
                  name={field.name}
                  required={field.required}
                  defaultValue={field.value}
                  rows={5}
                />
              ) : field.type === "select" ? (
                <select
                  name={field.name}
                  required={field.required}
                  defaultValue={field.value}
                >
                  {field.options?.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  name={field.name}
                  type={field.type ?? "text"}
                  required={field.required}
                  defaultValue={field.value}
                  autoComplete={
                    field.type === "password" ? "new-password" : "off"
                  }
                  maxLength={field.type === "password" ? 4096 : 998}
                />
              )}
            </label>
          ),
        )}
        {confirm && (
          <label className="check">
            <input type="checkbox" required name="confirm" value="yes" />
            {i18n.t("Confirm this change.")}
          </label>
        )}
        <button type="submit" className="button">
          {pending ? i18n.t("Saving…") : label}
        </button>
      </fieldset>
      {state.error && (
        <p className="notice error" role="alert">
          {i18n.error(state.error)}
        </p>
      )}
      {state.message && (
        <p className="notice" role="status">
          {i18n.error(state.message)}
        </p>
      )}
      {state.uncertain && (
        <p>
          {i18n.t(
            "Acceptance is uncertain. Inspect the message; do not send it again with a new request.",
          )}
        </p>
      )}
      {state.id && operation === "mailTestSend" && (
        <Link
          href={`/tenants/${customer}/mail/emails/${state.id}?connection=${connection}`}
        >
          {i18n.t("View message")}
        </Link>
      )}
      {secret && (
        <section className="notice">
          <p>{i18n.t("Save this secret now. It is shown only once.")}</p>
          <code style={{ overflowWrap: "anywhere" }}>{secret}</code>
          <button
            className="button secondary"
            type="button"
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(secret);
                setCopiedSecret(secret);
              } catch {
                setCopiedSecret(undefined);
              }
            }}
          >
            {copiedSecret === secret ? i18n.t("Copied") : i18n.t("Copy")}
          </button>
        </section>
      )}
    </form>
  );
}
export function CopyMailValue({ value }: { value: string }) {
  const i18n = useI18n();
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="button secondary"
      type="button"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(value);
          setCopied(true);
        } catch {
          setCopied(false);
        }
      }}
    >
      {copied ? i18n.t("Copied") : i18n.t("Copy")}
    </button>
  );
}
