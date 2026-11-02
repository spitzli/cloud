"use client";
import { useI18n } from "@webdock/i18n/react";
import { useActionState } from "react";
import { registerClient } from "./actions";
export function ClientForm() {
 const { t, error: translateError } = useI18n();
  const [state, action, pending] = useActionState(registerClient, {});
  return <form action={action}>
    <label htmlFor="client-name">{t("Client name")}</label><input id="client-name" name="name" required maxLength={100} placeholder={t("My MCP client")} />
    <label htmlFor="client-callback">{t("Exact callback URL")}</label><input id="client-callback" name="redirectURI" type="url" required placeholder="https://client.example/oauth/callback" />
    <p className="muted">{t("Copy this URL from your MCP client. Wildcards are not supported.")}</p>
    <label htmlFor="client-capability">{t("Access area")}</label><select id="client-capability" name="capability"><option value="registry">{t("Operator registry")}</option><option value="hosting">{t("Hosting (customer permissions apply)")}</option></select>
    <label className="check"><input name="write" type="checkbox" value="yes" /> {t("Allow changes in the selected area")}</label>
    <label className="check"><input name="offline" type="checkbox" value="yes" /> {t(" Allow automatic token renewal for remote work")}</label>
    <p className="muted">{t("Optional: stay connected during your working session. Signing out of Webdock or disabling this client stops access.")}</p>
    <label className="check"><input name="confidential" type="checkbox" value="yes" /> {t(" This client can securely store a client secret")}</label>
    <p className="muted">{t("Desktop and public clients normally use PKCE without a secret. You will still approve access during sign-in.")}</p>
    {state.error && <p role="alert">{translateError(state.error)}</p>}
    <button className="button" disabled={pending}>{pending ? t("Registering…") : t("Register MCP client")}</button>
    {state.clientID && <section aria-live="polite"><h2>{t("Client registered")}</h2><p>{t("Client ID")}</p><code>{state.clientID}</code>{state.clientSecret && <><p>{t("Client secret — save it now. It will not be shown again.")}</p><code style={{ overflowWrap: "anywhere" }}>{state.clientSecret}</code></>}</section>}
  </form>;
}
