"use client";
import { useI18n } from "@webdock/i18n/react";
import { useActionState, useState } from "react";
import { keyAction } from "./actions";
export function KeyForm({ customer, operator = false, consumerKey }: { customer: string; operator?: boolean; consumerKey?: string }) {
 const { t, error: translateError } = useI18n();
 const [state, submit, pending] = useActionState(keyAction, {});
 const [hidden, setHidden] = useState(false), [copied, setCopied] = useState("");
 return <form action={async form => { setHidden(false); setCopied(""); await submit(form); }} className="access-form" aria-busy={pending}>
  <input type="hidden" name="customer" value={customer} /><input type="hidden" name="action" value={consumerKey ? "revoke" : "create"} />
  <fieldset disabled={pending} style={{ border: 0, padding: 0, margin: 0, minWidth: 0, display: "grid", gap: 14 }}>
   {consumerKey ? <><input type="hidden" name="consumerKey" value={consumerKey} /><label className="check"><input type="checkbox" required name="confirm" value="yes" />{t("Revoke this credential. Applications using it will immediately lose access.")}</label></> : <>
    <label className="field">{t("Label")}<input name="label" required maxLength={100} placeholder={t("Production website")} autoComplete="off" /></label>
    <label className="check"><input type="checkbox" name="permissions" value="SEND_SMTP" defaultChecked />{t("SMTP sending")}</label>
    <label className="check"><input type="checkbox" name="permissions" value="SEND_API" defaultChecked />{t("API sending")}</label>
    {operator && <label className="check"><input type="checkbox" name="permissions" value="APIS" />{t("Provider administration (operator only)")}</label>}
    <label className="field">{t("Allowed IP addresses (optional)")}<textarea name="ips" rows={3} maxLength={4600} placeholder="203.0.113.10, 2001:db8::10" /><span className="muted">{t("Individual IPv4 or IPv6 addresses, separated by commas or spaces. Blank allows any IP address.")}</span></label>
   </>}
   <button type="submit" className="button secondary">{pending ? t("Working…") : consumerKey ? t("Revoke credential") : t("Create credential")}</button>
  </fieldset>
  {state.error && <p className="notice error" role="alert">{translateError(state.error)}</p>}{state.message && <p className="notice" role="status">{t(state.message)}</p>}
  {state.created && !hidden && !pending && <section className="notice"><h3>{t("Save your new credential")}</h3><p>{t("This secret is shown only now. Store it securely before leaving or dismissing this panel.")}</p><dl><dt>{t("Consumer key")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{state.created.consumerKey}</code></dd><dt>{t("Consumer secret")}</dt><dd style={{ overflowWrap: "anywhere" }}><code>{state.created.consumerSecret}</code></dd></dl><button className="button secondary" type="button" onClick={async () => { try { await navigator.clipboard.writeText(`Consumer key: ${state.created!.consumerKey}\nConsumer secret: ${state.created!.consumerSecret}`); setCopied("Copied."); } catch { setCopied("Copy failed. Select and copy the values above."); } }}>{t("Copy credentials")}</button> <button className="button secondary" type="button" onClick={() => setHidden(true)}>{t("Dismiss secret")}</button><span role="status">{t(copied)}</span></section>}
 </form>;
}
