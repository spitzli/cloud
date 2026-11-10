import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useI18n } from "@spitzli/i18n/react";
import { useApi } from "./context";
import { useHostingCommand } from "./hosting-client";
import { useBuildCommand } from "./build-client";
import { appsPageSchema, projectSchema } from "./hosting-contracts";
import { sourcesSchema } from "./build-contracts";
import { Failure, Loading, Status } from "./common";
import * as ui from "./ui";

/** Native hosting and Git inventory is separate from extension resource discovery. */
export function ProjectRuntime({ projectID, customer }: { projectID: string; customer: string }) {
  const api = useApi(), hosting = useHostingCommand(), git = useBuildCommand(), { t } = useI18n();
  const enabled = api.config.enabledFeatures.includes("hosting");
  const project = useQuery({ queryKey: ["hosting", "project", projectID], enabled, retry: false,
    queryFn: ({ signal }) => hosting({ action: "projects.get", projectID }, projectSchema, AbortSignal.any([signal, AbortSignal.timeout(15_000)])) });
  const matches = project.isSuccess && project.data.projectID === projectID && project.data.customerID === customer;
  const native = matches && project.data.configured && ["k3s", "docker"].includes(project.data.provider ?? "");
  const apps = useQuery({ queryKey: ["hosting", "apps", projectID, "", 1], enabled: native, retry: false,
    queryFn: ({ signal }) => hosting({ action: "apps.list", projectID, page: 1, limit: 20 }, appsPageSchema, AbortSignal.any([signal, AbortSignal.timeout(15_000)])) });
  const sources = useQuery({ queryKey: ["git-builds", projectID, "sources"], enabled: native, retry: false,
    queryFn: ({ signal }) => git({ action: "git.sources.list", projectID }, sourcesSchema, AbortSignal.any([signal, AbortSignal.timeout(15_000)])) });
  if (!enabled) return null;
  if (project.isPending) return <Loading>{t("Loading applications…")}</Loading>;
  if (project.error) return <Failure error={project.error} retry={() => void project.refetch()} />;
  if (!matches) return <Failure error={new Error(t("This project is not available in this workspace."))} />;
  return <div className="mb-6 grid gap-4">
    <section className={ui.card} aria-label={t("Applications")}>
      <header className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
        <h2 className={ui.sectionTitle}>{t("Applications")}</h2>
        <Link className={ui.button()} to="/hosting/projects/$projectID" params={{ projectID }}>{t("Manage applications")}</Link>
      </header>
      {!project.data.configured ? <p className="px-5 pb-5 text-sm text-muted">{t("Hosting is not configured for this project.")}</p>
        : project.data.provider === "vercel" ? <div className="px-5 pb-5"><span className={ui.badge()}>Vercel</span><p className="mt-2 text-sm text-muted">{t("Vercel manages deployment for this project.")}</p></div>
        : !native ? <p className="px-5 pb-5 text-sm text-muted">{t("Open application management to view this provider.")}</p>
        : apps.isPending ? <Loading>{t("Loading applications…")}</Loading>
        : apps.error ? <Failure error={apps.error} retry={() => void apps.refetch()} />
        : !apps.data.docs.length ? <p className="px-5 pb-5 text-sm text-muted">{t("No applications in this project yet.")}</p>
        : <ul>{apps.data.docs.map(app => <li key={app.id} className={ui.cardRow}>
          <Link className="font-medium" to="/hosting/apps/$appID" params={{ appID: app.id }}>{app.name}</Link>
          <div className="flex items-center gap-2"><span className="text-xs text-muted">{t("Recorded state")}</span><Status status={app.status} /></div>
        </li>)}</ul>}
      {apps.isSuccess && native && apps.data.totalPages > 1 ? <p className={ui.cardFooter}>{t("Open application management to see all applications.")}</p> : null}
    </section>
    {native ? <section className={ui.card} aria-label={t("Git repository")}>
      <header className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
        <h2 className={ui.sectionTitle}>{t("Git repository")}</h2>
        <Link className={ui.button()} to="/hosting/projects/$projectID/builds" params={{ projectID }}>{t("Git builds and releases")}</Link>
      </header>
      {sources.isPending ? <Loading>{t("Loading repository sources…")}</Loading>
        : sources.error ? <Failure error={sources.error} retry={() => void sources.refetch()} />
        : !sources.data.docs.length ? <p className="px-5 pb-5 text-sm text-muted">{t("No repository source is connected. Open Git builds and releases to connect one.")}</p>
        : <ul>{sources.data.docs.map(source => <li key={source.id} className={ui.cardRow}>
          <div><p className="font-medium [overflow-wrap:anywhere]">{source.repositoryName ?? t("Connected repository")}</p><p className="mt-1 text-xs text-muted [overflow-wrap:anywhere]">{t("Branch")}: {source.branch}</p></div>
          <span className={ui.badge(source.enabled ? "success" : "neutral")}>{source.enabled ? t("Enabled") : t("Disabled")}</span>
        </li>)}</ul>}
    </section> : null}
  </div>;
}
