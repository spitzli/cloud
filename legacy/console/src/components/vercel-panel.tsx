import { getStudioSession } from "@/lib/studio-client";
import {uiLabel} from "@/lib/ui-labels";

import { msgid } from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';
import Link from "next/link";
import {requireRegistryOperator as requireOperator} from "../lib/registry-page";
import { projectVercelStatus } from "../lib/vercel-view";
import type { VercelDeployment } from "@webdock/hosting-contracts/vercel";
async function Deployment({ deployment }: { deployment: VercelDeployment }) {
  const i18n = await getRequestI18n();

  return (
    <article className="vercel-deployment">
      <div className="section-heading">
        <strong>
          {deployment.target === "production" ? i18n.t("Production") : i18n.t("Preview")}
        </strong>
        <span
          className={
            "badge " + (deployment.status === "READY" ? "connected" : "")
          }
        >
          {i18n.t(uiLabel(deployment.status.toLowerCase()))}
        </span>
      </div>
      {deployment.commitMessage && <p>{deployment.commitMessage}</p>}
      <p className="muted">
        {deployment.branch || i18n.t("No Git branch recorded")}
        {deployment.commitSHA ? " · " + deployment.commitSHA.slice(0, 8) : ""}
      </p>
      <p className="muted">
        <time dateTime={deployment.createdAt || undefined}>
          {deployment.createdAt ? i18n.date(deployment.createdAt, {dateStyle:"medium",timeStyle:"short",timeZone:"UTC"}) + " UTC" : i18n.t("Not available")}
        </time>
      </p>
      {deployment.url && (
        <a
          className="small-link"
          href={deployment.url}
          target="_blank"
          rel="noreferrer"
        >{i18n.t("Open deployment ↗")}</a>
      )}
    </article>
  );
}
const messages: Record<string, string> = {
  setup: msgid("Connect Vercel in Integrations to view deployment information."),
  disconnected:
    msgid("The Vercel connection is not active. Connect it in Integrations."),
  unlinked:
    msgid("Link this Studio project to Vercel to see live deployment information."),
  unavailable:
    msgid("Vercel is temporarily unavailable. Your Studio records remain available."),
  forbidden:
    msgid("This project is not included in the Vercel integration’s selected access."),
  not_found: msgid("The linked Vercel project could not be found."),
};
export async function VercelProjectPanel({ projectID }: { projectID: string }) {
  if (!["enabled","draining"].includes((await getStudioSession())?.installation?.modules.hosting ?? "")) return null;
  const i18n = await getRequestI18n();

  const actor = await requireOperator();
  let result;
  try {
    result = await projectVercelStatus(actor, projectID);
  } catch {
    result = { state: "unavailable" as const };
  }
  if (result.state !== "ready" || !("snapshot" in result))
    return (
      <section className="panel">
        <h2>{i18n.t("Vercel")}</h2>
        <p>{i18n.t(messages[result.state] || messages.unavailable)}</p>
        <Link className="small-link" href="/integrations#vercel">{i18n.t("Manage connection")}</Link>
      </section>
    );
  const s = result.snapshot;
  const teamSlug = process.env.WEBDOCK_VERCEL_TEAM_SLUG;
  const dashboard =
    teamSlug && /^[a-z0-9-]+$/.test(teamSlug)
      ? `https://vercel.com/${encodeURIComponent(teamSlug)}/${encodeURIComponent(s.name)}`
      : "https://vercel.com/dashboard";
  return (
    <section
      className="panel vercel-panel"
      aria-labelledby="vercel-project-heading"
    >
      <div className="section-heading">
        <h2 id="vercel-project-heading">{i18n.t("Vercel · ")}{s.name}</h2>
        <a
          className="small-link"
          href={dashboard}
          target="_blank"
          rel="noreferrer"
        >{i18n.t("Open Vercel ↗")}</a>
      </div>
      <p className="muted">
        {s.framework || i18n.t("Framework not specified")}{i18n.t(" · Node.js")}{" "}
        {s.nodeVersion || i18n.t("not specified")}
      </p>
      <h3>{i18n.t("Current production")}</h3>
      {s.productionDeployment ? (
        <Deployment deployment={s.productionDeployment} />
      ) : (
        <p>{i18n.t("No production deployment is currently assigned.")}</p>
      )}
      <h3>{i18n.t("Domains")}</h3>
      {s.domains.length ? (
        <ul className="vercel-domains">
          {s.domains.map((domain) => (
            <li key={domain.name}>
              <span>{domain.name}</span>
              <small>
                {domain.verified ? i18n.t("Verified") : i18n.t("Verification pending")}
                {domain.redirect ? i18n.t(" · Redirects to ") + domain.redirect : ""}
              </small>
            </li>
          ))}
        </ul>
      ) : (
        <p>{i18n.t("No domains attached.")}</p>
      )}
      <h3>{i18n.t("Recent deployments")}</h3>
      {s.deployments.map((deployment) => (
        <Deployment key={deployment.id} deployment={deployment} />
      ))}
      {!s.deployments.length && <p>{i18n.t("No recent deployments.")}</p>}
      <div className="vercel-refresh">
        <small>{i18n.t("Checked ")}{i18n.date(s.checkedAt, {dateStyle:"medium",timeStyle:"short",timeZone:"UTC"}) + " UTC"}{i18n.t(". Read-only data from Vercel.")}</small>
        <a className="small-link" href={"/projects/" + projectID}>{i18n.t("Refresh Vercel data")}</a>
      </div>
    </section>
  );
}
