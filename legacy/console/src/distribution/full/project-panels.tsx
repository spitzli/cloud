import { Suspense } from "react";
import { getRequestI18n } from "@webdock/i18n/next";
import { ProjectApplications } from "@/components/hosting/project-applications";
import { VercelProjectPanel } from "@/components/vercel-panel";
export async function ProjectApplicationsPanel(props: {projectID: string; customerID: string}) {
  const i18n = await getRequestI18n();
  return <Suspense fallback={<p role="status">{i18n.t("Loading hosting status…")}</p>}><ProjectApplications {...props}/></Suspense>;
}
export async function ProjectVercelPanel({projectID}: {projectID: string}) {
  const i18n = await getRequestI18n();
  return <details className="workspace-disclosure"><summary>{i18n.t("Vercel")}</summary><Suspense fallback={<section className="panel"><h2>{i18n.t("Vercel")}</h2><p role="status">{i18n.t("Loading deployment information…")}</p></section>}><VercelProjectPanel projectID={projectID}/></Suspense></details>;
}
