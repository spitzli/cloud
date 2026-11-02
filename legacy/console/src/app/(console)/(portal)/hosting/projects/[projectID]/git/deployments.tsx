import { AreaNavigation } from "@/components/area-navigation";
import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingTable } from "@/components/hosting/table";
import { ApplicationEnvironment } from "@/components/hosting/environment";
import { HostingRefresh } from "@/components/hosting/refresh";

type Source = {
  id: string;
  connectionID: string;
  repositoryID: string;
  branch: string;
  rootDirectory: string;
  healthPath: string;
  recipe: "dockerfile" | "vercel";
  targetID: string;
  revision: number;
  enabled: boolean;
  autoPublish: boolean;
  buildProvider: "isolated" | "github-actions";
  workflowPath: string;
  artifactPrefix: string;
  buildEnvironmentNames: string[];
};
type Connection = {
  id: string;
  accountLogin: string;
  state: string;
  revision: number;
  repositories: {
    repositoryID: string;
    fullName: string;
    defaultBranch: string;
  }[];
};
type Build = {
  id: string;
  sourceSHA: string;
  status: string;
  createdAt: string;
  failureCode: string | null;
};
type Release = {
  id: string;
  artifactID: string;
  status: string;
  revision: number;
  approvedBy: string | null;
};
export default async function GitDeployments({
  params,
  searchParams, view = "overview",
}: {
  params: Promise<{ projectID: string }>; view?: "overview" | "setup";
  searchParams: Promise<{
    target?: string;
    connection?: string;
    page?: string;
    releasePage?: string;
  }>;
}) {
  const { projectID } = await params,
    query = await searchParams;
  const { t } = await getRequestI18n();
  const path = `/hosting/projects/${projectID}/git`;
  const sources = await hostingPageCall<{docs: (Source & {repositoryName: string})[]}>({action:'git.sources.list',projectID},path);
  if (!query.target && sources.docs.length > 1) return <>
    <h1>{t('Git deployments')}</h1><section className="panel">
      <h2>{t('Select an application')}</h2>
      <ul className="workspace-list">{sources.docs.map(source => <li key={source.id}>
        <Link href={`${path}${view === 'setup' ? '/setup' : ''}?target=${encodeURIComponent(source.targetID)}`}>
          {source.repositoryName} · {source.rootDirectory} · {source.targetID}
        </Link>
      </li>)}</ul>
    </section>
  </>;
  const targetID = query.target ?? sources.docs[0]?.targetID;
  const selectedSource = sources.docs.find(source => source.targetID === targetID);
  const targetQuery = targetID ? `?target=${encodeURIComponent(targetID)}` : '';
  const data = await hostingPageCall<{
    source: Source | null;
    customerID: string;
    canManage: boolean;
    canDeploy: boolean;
    targets: { id: string; name: string }[];
    candidateTargets: { id: string; name: string; mode: "byok" }[];
    canBindPlatform: boolean;
    blockers: string[];
  }>({ action: "git.source.get", projectID, targetID }, path + targetQuery);
  const setup = view === "setup";
  const [connections, builds, releases] = await Promise.all([
    setup ? hostingPageCall<{ docs: Connection[] }>(
      { action: "git.connections.list", customerID: data.customerID },
      path,
    ) : Promise.resolve({docs: [] as Connection[]}),
    !setup ? hostingPageCall<{ docs: Build[]; totalPages: number }>(
      {
        action: "git.builds.list",
        projectID,
        sourceID: selectedSource?.id,
        page: /^[1-9][0-9]{0,3}$/.test(query.page ?? "")
          ? Number(query.page)
          : 1,
        limit: 25,
      },
      path,
    ) : Promise.resolve({docs: [] as Build[], totalPages: 0}),
    !setup ? hostingPageCall<{ docs: Release[]; totalPages: number }>(
      {
        action: "git.releases.list",
        projectID,
        sourceID: selectedSource?.id,
        page: /^[1-9][0-9]{0,3}$/.test(query.releasePage ?? "")
          ? Number(query.releasePage)
          : 1,
        limit: 25,
      },
      path,
    ) : Promise.resolve({docs: [] as Release[], totalPages: 0}),
  ]);
  const active = connections.docs.filter((c) => c.state === "active");
  const connection =
    active.find(
      (c) => c.id === (query.connection ?? data.source?.connectionID),
    ) ?? active[0];
  const source = data.source;
  return (
    <>
      <h1>{t("Git deployments")}</h1>
      <AreaNavigation links={[{href:path+targetQuery,label:t("Activity"),exact:true},{href:path+"/setup"+targetQuery,label:t("Configuration"),exact:true}]}/>
      {!setup && (!data.source || data.blockers.length > 0) && <p><Link className="button secondary" href={path+"/setup"+targetQuery}>{t("Configure deployments")}</Link></p>}
      <HostingRefresh
        active={
          builds.docs.some((b) => ["queued", "running"].includes(b.status)) ||
          releases.docs.some((r) => ["queued", "deploying"].includes(r.status))
        }
      />
      <p>
        {t(
          "Pushes build the configured branch. Production publication requires approval of an exact artifact.",
        )}
      </p>
      {data.blockers.length > 0 && (
        <section className="panel" role="status">
          <h2>{t("Setup required")}</h2>
          <ul>
            {data.blockers.map((b) => (
              <li key={b}>{t(b)}</li>
            ))}
          </ul>
        </section>
      )}
      {setup && <>
      <section className="panel">
        <h2>{t("GitHub connection")}</h2>
        <HostingTable
          rows={connections.docs}
          rowKey={(c) => c.id}
          empty={t("No GitHub connection yet.")}
          columns={[
            { label: t("GitHub account"), render: (c) => c.accountLogin },
            { label: t("Status"), render: (c) => t(c.state) },
            {
              label: t("Actions"),
              render: (c) =>
                data.canManage && c.state !== "disconnected" ? (
                  <HostingForm
                    command={{
                      action: "git.connections.disconnect",
                      customerID: data.customerID,
                      connectionID: c.id,
                      revision: c.revision,
                    }}
                    label={t("Disconnect GitHub")}
                  />
                ) : null,
            },
          ]}
        />
        {data.canManage && (
          <form method="post" action="/api/hosting/git/connect">
            <input type="hidden" name="customerID" value={data.customerID} />
            <button className="button">{t("Connect GitHub")}</button>
          </form>
        )}
      </section>
      {data.canManage &&
        (data.canBindPlatform || data.candidateTargets?.length > 0) && (
          <section className="panel">
            <h2>{t("Bind Vercel target")}</h2>
            <p>
              {t(
                "Select an existing authorized Vercel project. Disable native Git builds and configure Frankfurt Functions before binding.",
              )}
            </p>
            <HostingForm
              command={{
                action: "git.targets.bind",
                projectID,
                targetID: "prj_unselected",
                mode: "byok",
              }}
              label={t("Verify and bind target")}
            >
              <label className="field">
                {t("Vercel project ID")}
                <input
                  name="targetID"
                  required
                  pattern="prj_[A-Za-z0-9]+"
                  list="vercel-candidates"
                />
                <datalist id="vercel-candidates">
                  {data.candidateTargets?.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name}
                    </option>
                  ))}
                </datalist>
              </label>
              <label className="field">
                {t("Connection")}
                <select name="mode">
                  <option value="byok">
                    {t("Customer Vercel connection")}
                  </option>
                  {data.canBindPlatform && (
                    <option value="platform">
                      {t("Platform Vercel connection")}
                    </option>
                  )}
                </select>
              </label>
            </HostingForm>
          </section>
        )}
      <section className="panel">
        <h2>{t("Deployment source")}</h2>
        {data.canManage && connection ? (
          <>
            {active.length > 1 && (
              <form method="get">
                {targetID && <input type="hidden" name="target" value={targetID}/>}
                <label className="field">
                  {t("GitHub account")}
                  <select name="connection" defaultValue={connection.id}>
                    {active.map((c) => (
                      <option value={c.id} key={c.id}>
                        {c.accountLogin}
                      </option>
                    ))}
                  </select>
                </label>
                <button className="button secondary">
                  {t("Select connection")}
                </button>
              </form>
            )}
            <HostingForm
              preserveRevision
              command={{
                action: "git.source.configure",
                projectID,
                connectionID: connection.id,
                repositoryID:
                  source?.repositoryID ??
                  connection.repositories[0]?.repositoryID ??
                  "1",
                branch: source?.branch ?? "main",
                rootDirectory: source?.rootDirectory ?? ".",
                healthPath: source?.healthPath ?? "/",
                recipe: source?.recipe ?? "dockerfile",
                targetID: source?.targetID ?? data.targets[0]?.id ?? "1",
                revision: source?.revision ?? 0,
                enabled: source?.enabled ?? true,
                autoPublish: source?.autoPublish ?? false,
                buildProvider: source?.buildProvider ?? "github-actions",
                workflowPath:
                  source?.workflowPath ??
                  ".github/workflows/production-build.yml",
                artifactPrefix: source?.artifactPrefix ?? "webdock",
              }}
              label={t("Save deployment source")}
            >
              <input type="hidden" name="connectionID" value={connection.id} />
              <label className="field">
                {t("Repository")}
                <select
                  name="repositoryID"
                  required
                  defaultValue={source?.repositoryID}
                >
                  {connection.repositories.map((r) => (
                    <option key={r.repositoryID} value={r.repositoryID}>
                      {r.fullName}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field">
                {t("Branch")}
                <input
                  name="branch"
                  required
                  maxLength={200}
                  defaultValue={
                    source?.branch ??
                    connection.repositories[0]?.defaultBranch ??
                    "main"
                  }
                />
              </label>
              <label className="field">
                {t("Root directory")}
                <input
                  name="rootDirectory"
                  required
                  maxLength={240}
                  defaultValue={source?.rootDirectory ?? "."}
                />
              </label>
              <label className="field">
                {t("Vercel healthcheck path")}
                <input
                  name="healthPath"
                  defaultValue={source?.healthPath ?? "/"}
                  maxLength={160}
                  required
                />
              </label>
              <p>
                {t(
                  "Use a public health endpoint that returns HTTP 200 without a redirect.",
                )}
              </p>
              <label className="field">
                {t("Build provider")}
                <select
                  name="buildProvider"
                  defaultValue={source?.buildProvider ?? "github-actions"}
                >
                  <option value="github-actions">GitHub Actions</option>
                  <option value="isolated">
                    {t("Isolated Webdock worker")}
                  </option>
                </select>
              </label>
              <label className="field">
                {t("GitHub Actions workflow")}
                <input
                  name="workflowPath"
                  defaultValue={
                    source?.workflowPath ??
                    ".github/workflows/production-build.yml"
                  }
                  maxLength={240}
                  required
                />
              </label>
              <label className="field">
                {t("Artifact name prefix")}
                <input
                  name="artifactPrefix"
                  defaultValue={source?.artifactPrefix ?? "webdock"}
                  maxLength={80}
                  required
                />
              </label>
              <p>
                {t(
                  "GitHub builds the artifact. Webdock verifies and publishes it. Artifact names end with the exact commit SHA.",
                )}
              </p>
              <label className="field">
                {t("Build recipe")}
                <select
                  name="recipe"
                  defaultValue={source?.recipe ?? "dockerfile"}
                >
                  <option value="dockerfile">Dockerfile</option>
                  <option value="vercel">Vercel</option>
                </select>
              </label>
              <label className="field">
                {t("Deployment target")}
                <select
                  name="targetID"
                  required
                  defaultValue={source?.targetID}
                >
                  {data.targets.map((target) => (
                    <option value={target.id} key={target.id}>
                      {target.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  name="enabled"
                  value="yes"
                  defaultChecked={source?.enabled ?? true}
                />
                {t("Build pushes automatically")}
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  name="autoPublish"
                  value="yes"
                  defaultChecked={source?.autoPublish ?? false}
                />
                {t("Publish production automatically after checks pass")}
              </label>
              <ApplicationEnvironment
                build
                names={source?.buildEnvironmentNames ?? []}
              />
            </HostingForm>
          </>
        ) : (
          <p>
            {source
              ? `${source.branch} · ${source.rootDirectory} · ${source.recipe}`
              : t("Connect a repository to configure Git deployments.")}
          </p>
        )}
        {!data.targets.length && (
          <p>
            {t(
              "Configure a deployment target before enabling Git deployments.",
            )}
          </p>
        )}
      </section>
      </>}
      {!setup && <>
      <section className="panel">
        <h2>{t("Builds")}</h2>
        {source?.enabled &&
          data.canDeploy &&
          source.buildProvider !== "github-actions" && (
            <HostingForm
              command={{
                action: "git.builds.request",
                projectID,
                targetID: source.targetID,
                idempotencyKey: randomUUID(),
              }}
              label={t("Build latest commit")}
            />
          )}
        {source?.buildProvider === "github-actions" && (
          <p>
            {t(
              "Push to the configured branch or start a new workflow in GitHub Actions. Re-running an existing run is not supported.",
            )}
          </p>
        )}
        <HostingTable
          rows={builds.docs}
          rowKey={(b) => b.id}
          empty={t("No builds yet.")}
          columns={[
            {
              label: t("Commit"),
              render: (b) => (
                <Link href={`${path}/builds/${b.id}`}>
                  <code>{b.sourceSHA.slice(0, 12)}</code>
                </Link>
              ),
            },
            { label: t("Status"), render: (b) => t(b.status) },
            {
              label: t("Actions"),
              render: (b) =>
                data.canDeploy && ["queued", "running"].includes(b.status) ? (
                  <HostingForm
                    command={{
                      action: "git.builds.cancel",
                      projectID,
                      buildID: b.id,
                    }}
                    label={t("Cancel build")}
                  />
                ) : null,
            },
          ]}
        />
        {builds.totalPages > 1 && (
          <nav aria-label={t("Build pages")}>
            {Array.from(
              { length: Math.min(builds.totalPages, 100) },
              (_, i) => (
                <Link
                  key={i}
                  href={`${path}?${new URLSearchParams({page: String(i + 1), ...(targetID ? {target: targetID} : {})})}`}
                  style={{ marginRight: "0.75rem" }}
                >
                  {i + 1}
                </Link>
              ),
            )}
          </nav>
        )}
      </section>
      <section className="panel">
        <h2>{t("Releases")}</h2>
        <p>
          {t(
            "Rollback reuses a retained artifact. It does not restore a database.",
          )}
        </p>
        <HostingTable
          rows={releases.docs}
          rowKey={(r) => r.id}
          empty={t("No releases yet.")}
          columns={[
            {
              label: t("Artifact"),
              render: (r) => <code>{r.artifactID}</code>,
            },
            { label: t("Status"), render: (r) => t(r.status) },
            {
              label: t("Actions"),
              render: (r) =>
                data.canDeploy &&
                [
                  "awaiting-approval",
                  "queued",
                  "ready",
                  "needs-reconciliation",
                ].includes(r.status) ? (
                  <HostingForm
                    command={
                      r.status === "needs-reconciliation"
                        ? {
                            action: "git.releases.reconcile",
                            projectID,
                            releaseID: r.id,
                            revision: r.revision,
                          }
                        : {
                            action:
                              r.status === "ready"
                                ? "git.releases.rollback"
                                : "git.releases.approve",
                            projectID,
                            releaseID: r.id,
                            revision: r.revision,
                            idempotencyKey: randomUUID(),
                          }
                    }
                    label={
                      r.status === "needs-reconciliation"
                        ? t("Reconcile release")
                        : r.status === "ready"
                          ? t("Request rollback")
                          : r.status === "queued"
                            ? t("Renew approval")
                            : t("Approve release")
                    }
                  />
                ) : null,
            },
          ]}
        />
        {releases.totalPages > 1 && (
          <nav aria-label={t("Release pages")}>
            {Array.from(
              { length: Math.min(releases.totalPages, 100) },
              (_, i) => (
                <Link
                  key={i}
                  href={`${path}?${new URLSearchParams({releasePage: String(i + 1), ...(targetID ? {target: targetID} : {})})}`}
                  style={{ marginRight: "0.75rem" }}
                >
                  {i + 1}
                </Link>
              ),
            )}
          </nav>
        )}
      </section>
      </>}
    </>
  );
}
