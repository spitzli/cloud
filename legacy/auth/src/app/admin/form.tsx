"use client";
import { authMessage } from "@/lib/i18n-feedback";
import { useI18n } from "@webdock/i18n/react";

import { useActionState } from "react";
import type { PlatformSettings } from "@/lib/platform";
import { adminAction, type AdminState } from "./actions";

function Feedback({ state }: { state: AdminState }) {
 const i18n = useI18n();
 const { error: translateError } = i18n;
  return <>{state.error && <p className="notice error" role="alert">{translateError(state.error)}</p>}{state.message && <p className="notice" role="status">{authMessage(state.message, i18n)}</p>}</>;
}

export function AdminForms({ settings, connection }: {
  settings: PlatformSettings;
  connection: { configured: boolean; consumerKeySuffix?: string };
}) {
 const { t } = useI18n();
  const [settingsState, settingsAction, saving] = useActionState(adminAction, {});
  const [credentialsState, credentialsAction, savingCredentials] = useActionState(adminAction, {});
  const [testState, testAction, testing] = useActionState(adminAction, {});
  const [clearState, clearAction, clearing] = useActionState(adminAction, {});
  return <div className="access-grid">
    <section className="auth-panel" aria-labelledby="settings-heading">
      <h2 id="settings-heading">{t("Platform settings")}</h2>
      <form className="access-form" action={settingsAction}>
        <input type="hidden" name="action" value="settings" />
        <label className="field">{t("Display name")}<input name="name" defaultValue={settings.name} required maxLength={80} /></label>
        <label className="field">{t("Support email")}<input name="supportEmail" type="email" defaultValue={settings.supportEmail} required maxLength={254} /></label>
        <p className="muted">{t("Used for account branding and authentication emails.")}</p>
        <h3>{t("CMS")}</h3>
        <label className="check"><input type="checkbox" name="cmsEnabled" value="yes" defaultChecked={settings.cmsEnabled} /> {t(" Enable CMS access for customers")}</label>
        <p className="muted">{t("Controls customer sign-in and editing access to connected content managers. Public websites and operator maintenance access stay available.")}</p>
        <h3>{t("Tenant Mail")}</h3>
        <label className="check"><input type="checkbox" name="mailEnabled" value="yes" defaultChecked={settings.mailEnabled} /> {t(" Enable Mail for tenants")}</label>
        <p className="muted">{t("Controls tenant Mail operations. Turning this off does not disable account verification, password reset or invitation emails.")}</p>
        <label className="field">{t("Assigned sending IPv4 address")}<input name="mailSendingIP" defaultValue={settings.mailSendingIP} placeholder="203.0.113.10" maxLength={15} aria-describedby="sending-ip-help" /></label>
        <p id="sending-ip-help" className="muted">{t("Use the sending IP assigned by turboSMTP. It is required before provisioning tenant Mail accounts.")}</p>
        <label className="field">{t("Default email limit")}<input name="mailDefaultLimit" type="number" min={0} max={1000000000} step={1} defaultValue={settings.mailDefaultLimit} required /></label>
        <label className="field">{t("Mail region")}<select name="mailRegion" defaultValue={settings.mailRegion}><option value="eu">{t("Europe")}</option><option value="global">{t("Global")}</option></select></label>
        <Feedback state={settingsState} />
        <button className="button" disabled={saving}>{saving ? t("Saving…") : t("Save settings")}</button>
      </form>
    </section>
    <section className="auth-panel" aria-labelledby="mail-heading">
      <h2 id="mail-heading">{t("Mail provider")}</h2>
      <p>{t("Connect the turboSMTP master account used to manage tenant Mail accounts.")}</p>
      <p><a href="https://dashboard.serversmtp.com" target="_blank" rel="noopener noreferrer">{t("Open turboSMTP dashboard ↗")}</a></p>
      <p className="notice" role="status">{connection.configured ? t("Credentials configured{value1}", {value1: connection.consumerKeySuffix ? t(" · Key ending in {value1}", {value1: connection.consumerKeySuffix}) : ""}) : t("No credentials configured")}</p>
      <form className="access-form" action={credentialsAction} autoComplete="off">
        <input type="hidden" name="action" value="credentials" />
        <label className="field">{t("Consumer key")}<input name="consumerKey" type="password" autoComplete="new-password" required /></label>
        <label className="field">{t("Consumer secret")}<input name="consumerSecret" type="password" autoComplete="new-password" required /></label>
        <p className="muted">{t("Saved credentials are never displayed. Leave these fields untouched to keep the current credentials; saving platform settings does not replace them.")}</p>
        <Feedback state={credentialsState} />
        <button className="button" disabled={savingCredentials || clearing}>{savingCredentials ? t("Saving…") : t("Save credentials")}</button>
      </form>
      <form className="access-form" action={testAction}>
        <input type="hidden" name="action" value="test" />
        <p className="muted">{t("Test the saved connection by reading the provider account list. No email is sent.")}</p>
        <Feedback state={testState} />
        <button className="button secondary" disabled={!connection.configured || testing || savingCredentials || clearing}>{testing ? t("Testing…") : t("Test connection")}</button>
      </form>
      {connection.configured && <form className="access-form" action={clearAction}>
        <input type="hidden" name="action" value="clear" />
        <h3>{t("Remove credentials")}</h3>
        <p className="muted">{t("Removing master credentials disables new live Mail operations. Existing provider accounts are not deleted. Revoke the key separately in turboSMTP if it should no longer be valid.")}</p>
        <label className="check"><input name="confirm" type="checkbox" value="yes" required /> {t(" I want to remove the saved Mail credentials.")}</label>
        <Feedback state={clearState} />
        <button className="button secondary" disabled={clearing || savingCredentials}>{clearing ? t("Removing…") : t("Remove credentials")}</button>
      </form>}
      {!connection.configured && <Feedback state={clearState} />}
    </section>
  </div>;
}
