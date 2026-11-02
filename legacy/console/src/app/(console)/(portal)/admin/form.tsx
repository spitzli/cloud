"use client";
import { useI18n } from '@webdock/i18n/react';


import { useActionState } from "react";
import type { PlatformSettings } from "@/lib/platform";
import { adminAction, type AdminState } from "./actions";

function Feedback({ state }: { state: AdminState }) {
  const i18n = useI18n();

  return (
    <>
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
    </>
  );
}

export function AdminForms({
  settings,
  connection,
}: {
  settings: PlatformSettings;
  connection: { configured: boolean; consumerKeySuffix?: string } | null;
}) {
  const i18n = useI18n();

  const [settingsState, settingsAction, saving] = useActionState(
    adminAction,
    {},
  );
  const [credentialsState, credentialsAction, savingCredentials] =
    useActionState(adminAction, {});
  const [testState, testAction, testing] = useActionState(adminAction, {});
  const [clearState, clearAction, clearing] = useActionState(adminAction, {});
  return (
    <div className="access-grid">
      <section className="auth-panel" aria-labelledby="settings-heading">
        <h2 id="settings-heading">{i18n.t("Platform settings")}</h2>
        <form className="access-form" action={settingsAction}>
          <input type="hidden" name="action" value="settings" />
          <label className="field">{i18n.t("Display name")}<input
              name="name"
              defaultValue={settings.name}
              required
              maxLength={80}
            />
          </label>
          <label className="field">{i18n.t("Support email")}<input
              name="supportEmail"
              type="email"
              defaultValue={settings.supportEmail}
              required
              maxLength={254}
            />
          </label>
          <p className="muted">{i18n.t("Used for account branding and authentication emails.")}</p>
          <h3>{i18n.t("CMS")}</h3>
          <label className="check">
            <input
              type="checkbox"
              name="cmsEnabled"
              value="yes"
              defaultChecked={settings.cmsEnabled}
            />{" "}{i18n.t("Enable CMS access for customers")}</label>
          <p className="muted">{i18n.t("Controls customer sign-in and editing access to connected content managers. Public websites and operator maintenance access stay available.")}</p>
          <h3>{i18n.t("Tenant Mail")}</h3>
          <label className="check">
            <input
              type="checkbox"
              name="mailEnabled"
              value="yes"
              defaultChecked={settings.mailEnabled}
            />{" "}{i18n.t("Enable Mail for tenants")}</label>
          <p className="muted">{i18n.t("Controls tenant Mail operations. Turning this off does not disable account verification, password reset or invitation emails.")}</p>
          <h3>{i18n.t("Mail gateway controls")}</h3>
          <label className="check"><input type="checkbox" name="mailGatewayEnabled" value="yes" defaultChecked={settings.mailGatewayEnabled}/>{i18n.t("Enable Mail API for managed and own accounts")}</label>
          <label className="check"><input type="checkbox" name="mailDispatchEnabled" value="yes" defaultChecked={settings.mailDispatchEnabled}/>{i18n.t("Enable Mail dispatch")}</label>
          <label className="check"><input type="checkbox" name="mailWebhooksEnabled" value="yes" defaultChecked={settings.mailWebhooksEnabled}/>{i18n.t("Enable customer webhook delivery")}</label>
          <p className="muted">{i18n.t("These controls do not stop authentication emails or direct SMTP. Accepted work and history are retained while dispatch is paused.")}</p>
          <label className="field">{i18n.t("Assigned sending IPv4 address")}<input
              name="mailSendingIP"
              defaultValue={settings.mailSendingIP}
              placeholder="203.0.113.10"
              maxLength={15}
              aria-describedby="sending-ip-help"
            />
          </label>
          <p id="sending-ip-help" className="muted">{i18n.t("Use the sending IP assigned by turboSMTP. It is required before provisioning tenant Mail accounts.")}</p>
          <label className="field">{i18n.t("Default email limit")}<input
              name="mailDefaultLimit"
              type="number"
              min={0}
              max={1000000000}
              step={1}
              defaultValue={settings.mailDefaultLimit}
              required
            />
          </label>
          <label className="field">{i18n.t("Mail region")}<select name="mailRegion" defaultValue={settings.mailRegion}>
              <option value="eu">{i18n.t("Europe")}</option>
              <option value="global">{i18n.t("Global")}</option>
            </select>
          </label>
          <Feedback state={settingsState} />
          <button className="button" disabled={saving}>
            {saving ? i18n.t("Saving…") : i18n.t("Save settings")}
          </button>
        </form>
      </section>
      {connection && <section className="auth-panel" aria-labelledby="mail-heading">
        <h2 id="mail-heading">{i18n.t("Mail provider")}</h2>
        <p>{i18n.t("Connect the turboSMTP master account used to manage tenant Mail accounts.")}</p>
        <p>
          <a
            href="https://dashboard.serversmtp.com"
            target="_blank"
            rel="noopener noreferrer"
          >{i18n.t("Open turboSMTP dashboard ↗")}</a>
        </p>
        <p className="notice" role="status">
          {connection.configured
            ? i18n.t("Credentials configured") + (connection.consumerKeySuffix ? i18n.t(" · Key ending in {suffix}",{suffix:connection.consumerKeySuffix}) : "")
            : i18n.t("No credentials configured")}
        </p>
        <details open={!connection.configured}>
          <summary>
            {connection.configured
              ? i18n.t("Replace provider credentials")
              : i18n.t("Connect provider credentials")}
          </summary>
          <form
            className="access-form"
            action={credentialsAction}
            autoComplete="off"
          >
            <input type="hidden" name="action" value="credentials" />
            <label className="field">{i18n.t("Consumer key")}<input
                name="consumerKey"
                type="password"
                autoComplete="new-password"
                required
              />
            </label>
            <label className="field">{i18n.t("Consumer secret")}<input
                name="consumerSecret"
                type="password"
                autoComplete="new-password"
                required
              />
            </label>
            <p className="muted">{i18n.t("Saved credentials are never displayed. Leave these fields untouched to keep the current credentials; saving platform settings does not replace them.")}</p>
            <Feedback state={credentialsState} />
            <button className="button" disabled={savingCredentials || clearing}>
              {savingCredentials ? i18n.t("Saving…") : i18n.t("Save credentials")}
            </button>
          </form>
        </details>
        <form className="access-form" action={testAction}>
          <input type="hidden" name="action" value="test" />
          <p className="muted">{i18n.t("Test the saved connection by reading the provider account list. No email is sent.")}</p>
          <Feedback state={testState} />
          <button
            className="button secondary"
            disabled={
              !connection.configured || testing || savingCredentials || clearing
            }
          >
            {testing ? i18n.t("Testing…") : i18n.t("Test connection")}
          </button>
        </form>
        {connection.configured && (
          <details>
            <summary>{i18n.t("Remove provider connection")}</summary>
            <form className="access-form" action={clearAction}>
              <input type="hidden" name="action" value="clear" />
              <h3>{i18n.t("Remove credentials")}</h3>
              <p className="muted">{i18n.t("Removing master credentials disables new live Mail operations. Existing provider accounts are not deleted. Revoke the key separately in turboSMTP if it should no longer be valid.")}</p>
              <label className="check">
                <input name="confirm" type="checkbox" value="yes" required />{i18n.t(" I want to remove the saved Mail credentials.")}</label>
              <Feedback state={clearState} />
              <button
                className="button secondary"
                disabled={clearing || savingCredentials}
              >
                {clearing ? i18n.t("Removing…") : i18n.t("Remove credentials")}
              </button>
            </form>
          </details>
        )}
        {!connection.configured && <Feedback state={clearState} />}
      </section>}
    </div>
  );
}
