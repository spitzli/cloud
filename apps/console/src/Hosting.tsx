import { clusterLabel } from "./cluster-label";
import { useI18n } from "@webdock/i18n/react";
import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { HostingCreate } from "./HostingCreate";
import { useQuery } from "@tanstack/react-query";
import { formatResource } from "@webdock/hosting-contracts";
import { useApi, useSession } from "./context";
import { useHostingCommand } from "./hosting-client";
import {
  appsPageSchema,
  projectPageSchema,
  projectSchema,
  tenantsSchema,
} from "./hosting-contracts";
import { Empty, Failure, Loading } from "./common";
import * as ui from "./ui";
const recordLink = "inline-block py-0.5 font-medium";
const recordMeta = `block text-muted ${ui.mono}`;
export function HostingDirectory({
  customerID,
}: {
  customerID?: string | undefined;
}) {
  const { t } = useI18n();
  const api = useApi();
  const call = useHostingCommand();
  const navigate = useNavigate();
  const enabled = api.config.enabledFeatures.includes("hosting");
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const tenants = useQuery({
    queryKey: ["tenants"],
    queryFn: ({ signal }) =>
      api.get("/api/console/tenants", tenantsSchema, signal),
    enabled,
  });
  const projects = useQuery({
    queryKey: ["hosting", "projects", customerID, search, page],
    queryFn: ({ signal }) =>
      call(
        { action: "projects.list", customerID, search, page, limit: 20 },
        projectPageSchema,
        signal,
      ),
    enabled: enabled && Boolean(customerID),
  });
  if (!enabled)
    return (
      <Empty title="Hosting is unavailable">
        Hosting is not enabled for this installation.
      </Empty>
    );
  if (tenants.isPending) return <Loading>Loading workspaces…</Loading>;
  if (tenants.error)
    return (
      <Failure error={tenants.error} retry={() => void tenants.refetch()} />
    );
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Applications</h1>
          <p className={ui.pageLead}>
            Choose a workspace and project to manage its applications.
          </p>
        </div>
      </div>
      <label className={ui.cx(`${ui.field} mb-6 max-w-[420px]`)}>
        Workspace
        <select
          className={ui.input}
          aria-label="Workspace"
          value={customerID ?? ""}
          onChange={(event) => {
            setPage(1);
            setSearch("");
            void navigate({
              to: "/hosting",
              search: { customer: event.target.value || undefined },
            });
          }}
        >
          <option value="">Choose a workspace</option>
          {tenants.data.map((tenant) => (
            <option key={tenant.id} value={tenant.id}>
              {tenant.name}
            </option>
          ))}
        </select>
      </label>
      {!customerID ? (
        <Empty title="Choose a workspace">
          Your available hosting projects will appear here.
        </Empty>
      ) : (
        <>
          <HostingSearch
            search={search}
            onSearch={(value) => {
              setSearch(value);
              setPage(1);
            }}
          />
          {projects.isPending ? (
            <Loading>Loading hosting projects…</Loading>
          ) : projects.error ? (
            <Failure
              error={projects.error}
              retry={() => void projects.refetch()}
            />
          ) : projects.data.docs.length === 0 ? (
            <Empty title="No hosting projects">
              No configured hosting project matches this workspace and search.
            </Empty>
          ) : (
            <>
              <div className={ui.tableFrame}>
                <table className={ui.table}>
                  <thead>
                    <tr>
                      <th scope="col" className={ui.cx(`${ui.th} w-1/2`)}>Project</th>
                      <th scope="col" className={ui.th}>Management</th>
                      <th scope="col" className={ui.th}>Cluster</th>
                    </tr>
                  </thead>
                  <tbody>
                    {projects.data.docs.map((project) => (
                      <tr key={project.projectID} className={ui.tr}>
                        <td className={ui.cx(`${ui.td} min-w-[170px]`)}>
                          <Link
                            className={`${recordLink} ${ui.rowLink}`}
                            to="/hosting/projects/$projectID"
                            params={{ projectID: project.projectID }}
                          >
                            {project.name}
                          </Link>
                          <span className={recordMeta}>
                            {project.projectID}
                          </span>
                        </td>
                        <td className={ui.td}>
                          {project.mode === "managed"
                            ? "Managed"
                            : "Self-service"}
                        </td>
                        <td className={ui.cx(`${ui.td} text-muted`)}>{project.provider === "vercel" ? "Vercel" : (project.clusterName ? clusterLabel(project.clusterName) : project.clusterID ? t("Managed cluster") : t("No cluster assigned"))}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <HostingPagination
                page={projects.data.page}
                totalPages={projects.data.totalPages}
                onPage={setPage}
              />
            </>
          )}
        </>
      )}
    </section>
  );
}
export function HostingProject({ projectID }: { projectID: string }) {
  const { t } = useI18n();
  const api = useApi();
  const session = useSession();
  const call = useHostingCommand();
  const enabled = api.config.enabledFeatures.includes("hosting");
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const project = useQuery({
    queryKey: ["hosting", "project", projectID],
    queryFn: ({ signal }) =>
      call({ action: "projects.get", projectID }, projectSchema, signal),
    enabled,
  });
  const apps = useQuery({
    queryKey: ["hosting", "apps", projectID, search, page],
    queryFn: ({ signal }) =>
      call(
        { action: "apps.list", projectID, search, page, limit: 20 },
        appsPageSchema,
        signal,
      ),
    enabled: enabled && project.data?.configured === true && project.data.provider !== "vercel",
  });
  if (!enabled)
    return (
      <Empty title="Hosting is unavailable">
        Hosting is not enabled for this installation.
      </Empty>
    );
  if (project.isPending) return <Loading>Loading project…</Loading>;
  if (project.error)
    return (
      <Failure error={project.error} retry={() => void project.refetch()} />
    );
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link
            className={ui.backLink}
            to="/hosting"
            search={{ customer: project.data.customerID }}
          >
            {project.data.customerName}
          </Link>
          <h1 className={ui.pageTitle}>{project.data.name}</h1>
          <p className={ui.pageLead}>{project.data.provider === "vercel" ? t("Vercel manages deployment for this project.") : "Applications in this project."}</p>
          {project.data.provider !== "vercel" && <Link className="mt-2 inline-block text-sm underline" to="/hosting/projects/$projectID/builds" params={{projectID}}>Git builds and releases</Link>}
        </div>
        <button
          className={ui.button()}
          disabled={apps.isFetching}
          onClick={() => {
            void project.refetch();
            if (project.data.provider !== "vercel") void apps.refetch();
          }}
        >
          Refresh
        </button>
      </div>
      {!project.data.configured ? (
        <Empty title="Hosting is not configured">
          This project has no hosting configuration. An operator must configure
          it before applications can be managed.
        </Empty>
      ) : project.data.provider === "vercel" ? (
        <Empty title={t("Hosted on Vercel")}>
          {t("Deployment information for this provider is not available here yet.")}
        </Empty>
      ) : (
        <>
          {apps.data?.canManage && !(session.user.preview && !session.user.acting) ? (
            <HostingCreate
              projectID={projectID}
              subscriptionRevision={apps.data.subscriptionRevision}
              publicSitePolicy={project.data.publicSitePolicy}
            />
          ) : null}
          <HostingSearch
            search={search}
            onSearch={(value) => {
              setSearch(value);
              setPage(1);
            }}
          />
          {apps.isPending ? (
            <Loading>Loading applications…</Loading>
          ) : apps.error ? (
            <Failure error={apps.error} retry={() => void apps.refetch()} />
          ) : apps.data.docs.length === 0 ? (
            <Empty title="No applications">
              No applications match this project and search.
            </Empty>
          ) : (
            <>
              <div className={ui.tableFrame}>
                <table className={ui.table}>
                  <thead>
                    <tr>
                      <th scope="col" className={ui.cx(`${ui.th} w-1/2`)}>Application</th>
                      <th scope="col" className={ui.th}>Recorded state</th>
                      <th scope="col" className={ui.th}>Configured resources</th>
                    </tr>
                  </thead>
                  <tbody>
                    {apps.data.docs.map((app) => (
                      <tr key={app.id} className={ui.tr}>
                        <td className={ui.cx(`${ui.td} min-w-[170px]`)}>
                          <Link
                            className={`${recordLink} ${ui.rowLink}`}
                            to="/hosting/apps/$appID"
                            params={{ appID: app.id }}
                          >
                            {app.name}
                          </Link>
                          <span className={recordMeta}>{app.id}</span>
                        </td>
                        <td className={ui.td}>
                          <span className={ui.cx(`${ui.badge(ui.statusTone(app.status))} normal-case`)}>{app.status}</span>
                          <span className="mt-1 block text-xs text-muted">
                            Observed revision {app.observedRevision} / desired{" "}
                            {app.revision}
                          </span>
                        </td>
                        <td className={ui.cx(`${ui.td} text-muted`)}>
                          {app.spec.replicas} replicas ·{" "}
                          {formatResource("memoryBytes", app.spec.memoryBytes)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <HostingPagination
                page={apps.data.page}
                totalPages={apps.data.totalPages}
                onPage={setPage}
              />
            </>
          )}
        </>
      )}
    </section>
  );
}
function HostingSearch({
  search,
  onSearch,
}: {
  search: string;
  onSearch: (value: string) => void;
}) {
  return (
    <form
      className="mb-4 flex flex-wrap items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        const value = new FormData(event.currentTarget).get("search");
        if (typeof value === "string") onSearch(value.trim());
      }}
    >
      <label className="w-full sm:w-[340px]">
        <input
          className={ui.cx(`${ui.input} min-h-9`)}
          key={search}
          name="search"
          aria-label="Search by name"
          defaultValue={search}
          maxLength={160}
          placeholder="Search by name…"
        />
      </label>
      <button className={ui.button()}>Search</button>
    </form>
  );
}
export function HostingPagination({
  page,
  totalPages,
  onPage,
}: {
  page: number;
  totalPages: number;
  onPage: (page: number) => void;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 py-3.5 text-[13px]">
      <span className="text-muted">
        Page {page} of {Math.max(1, totalPages)}
      </span>
      <div className="flex gap-2">
        <button
          className={ui.button("quiet")}
          disabled={page <= 1}
          onClick={() => {
            onPage(page - 1);
          }}
        >
          Previous
        </button>
        <button
          className={ui.button("quiet")}
          disabled={page >= totalPages}
          onClick={() => {
            onPage(page + 1);
          }}
        >
          Next
        </button>
      </div>
    </div>
  );
}
