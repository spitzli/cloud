import { IntentLink } from '@/components/intent-link';
import {getRequestI18n} from '@webdock/i18n/next';
import type { Metadata } from 'next';

export async function generateMetadata():Promise<Metadata>{const {t}=await getRequestI18n();return {
  title: t("Vercel integration — Spitzli Cloud Studio"),
  description: t("Connect selected Vercel projects to Spitzli Cloud Studio with read-only access."),
  alternates: { canonical: '/integrations/vercel' },
  openGraph: { title: t("Vercel integration — Spitzli Cloud Studio"), url: '/integrations/vercel' },
};}

export default async function VercelGuide() {
  const {t}=await getRequestI18n();
  return <article>
    <p className="subtle-label">{t("Spitzli Cloud Studio · Integration guide")}</p>
    <h1>{t("Vercel, in Studio.")}</h1>
    <p>{t("View hosting information alongside your Spitzli Cloud project records: current production release, recent deployments, Git branch and commit, runtime settings, and domain verification.")}</p>
    <h2>{t("Connect your projects")}</h2>
    <ol>
      <li>{t("Sign in to")} <a href="https://console.webdock.dev/integrations">{t("Studio → Integrations")}</a> {t("with an authorised platform operator account.")}</li>
      <li>{t("Choose Connect Vercel. Start here so Studio can securely associate the installation with your session.")}</li>
      <li>{t("In Vercel, select your team and the projects to share. Review the four read permissions: projects, deployments, domains and integration configuration.")}</li>
      <li>{t("Finish the installation and return to Studio. Match an active Studio project to its Vercel project in Integrations.")}</li>
      <li>{t("Open the project in Studio to view hosting details. Refresh to request the latest available status.")}</li>
    </ol>
    <h2>{t("Access and troubleshooting")}</h2>
    <p>{t("This private integration is available to the configured team and authorised operators. It reads hosting information; it cannot deploy, change domains or read environment-variable values. If access is denied, check that the project was selected during installation. If a connection has been revoked or expired, reconnect from Studio. Vercel outages can temporarily prevent status updates.")}</p>
    <h2>{t("Disconnect")}</h2>
    <p>{t("Disconnect in Studio to delete its stored access token. Also uninstall Spitzli Cloud Studio in your Vercel team’s integration settings to revoke permission at Vercel. Project mappings and audit records remain; contact us if you need them removed.")}</p>
    <h2>{t("Support")}</h2>
    <p>{t("Email support at")} <a href="mailto:dominik@spitzli.dev">{"dominik@spitzli.dev"}</a> {t("with the affected project and the visible error. Never include passwords, access tokens or environment-variable values.")}</p>
    <p>{t("Read the")} <IntentLink href="/terms">{t("terms of use")}</IntentLink> {t("and the")} <IntentLink href="/privacy">{t("privacy notice")}</IntentLink> {t("before connecting.")}</p>
  </article>;
}
