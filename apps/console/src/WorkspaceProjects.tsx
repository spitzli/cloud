import { useProjectIntent } from "./project-intent";
import { tenantsSchema } from "./workspace-contracts";
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { z } from "zod";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading } from "./common";
import { formText } from "./form";
import * as ui from "./ui";
import { useWorkspacePlugins } from "./plugins/workspace";
import { Attention } from "./Notices";
import { WorkspaceUsageSummary } from "./Usage";
import { WorkspaceAvatar } from "./WorkspaceAvatar";
import { FolderSimpleIcon } from "@phosphor-icons/react/dist/csr/FolderSimple";
import { MagnifyingGlassIcon } from "@phosphor-icons/react/dist/csr/MagnifyingGlass";
import { ArrowClockwiseIcon } from "@phosphor-icons/react/dist/csr/ArrowClockwise";


const projectsSchema = z.array(z.object({ id: z.string(), name: z.string(), status: z.string() }));

/** A workspace's projects. Owners, admins and operators start new ones here. */
export function WorkspaceProjects({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const client = useQueryClient();
  const navigate = useNavigate();
  const intent=useProjectIntent();
  const tenants = useQuery({ queryKey: ["tenants"], queryFn: ({ signal }) => api.get("/api/console/tenants", tenantsSchema, signal) });
  const projects = useQuery({
    queryKey: ["workspace-projects", id],
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${encodeURIComponent(id)}/projects`, projectsSchema, signal),
  });
  const workspace = useWorkspacePlugins(id);
  const [adding, setAdding] = useState(false);
  const [search, setSearch] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([projects.refetch(),
        ...[["usage", id], ["hosting", "usage", id], ["notifications", id], ...(projects.data ?? []).map((project) => ["project-plugins", session.session.id, project.id])].map((queryKey) => client.invalidateQueries({ queryKey })),
      ]);
    } finally { setRefreshing(false); }
  };
  const create = useMutation({
    mutationFn: (name: string) =>
      api.write(`/api/console/tenant/${encodeURIComponent(id)}/projects`, "POST", { name }, z.object({ id: z.string() })),
    onSuccess: async (project) => {
      await client.invalidateQueries({ queryKey: ["workspace-projects", id] });
      await navigate({ to: "/workspaces/$customer/projects/$id", params: { customer: id, id: project.id } });
    },
  });
  const tenant = tenants.data?.find((t) => t.id === id);
  const operator = session.user.role === "operator" && !(session.user.preview && !session.user.acting);
  const canCreate = !(session.user.preview && !session.user.acting) && (operator || tenant?.role === "owner" || tenant?.role === "admin");
  if (projects.isPending) return <Loading>Loading projects…</Loading>;
  if (projects.error) return <Failure error={projects.error} retry={() => void projects.refetch()} />;
  const filtered = projects.data.filter((project) => project.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()));
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          {tenants.data && tenants.data.length > 1 ? <Link className={ui.backLink} to="/$collection" params={{ collection: "tenants" }}><ArrowLeftIcon />All workspaces</Link> : null}
          <div className="flex items-center gap-3">
            <WorkspaceAvatar name={tenant?.name ?? "Workspace"} icon={tenant?.icon} size="size-10 text-base" />
            <h1 className={ui.pageTitle}>{tenant?.name ?? "Workspace"}</h1>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <button type="button" aria-label="Refresh workspace" className={ui.button("quiet", "icon")} disabled={refreshing} onClick={() => { void refresh(); }}><ArrowClockwiseIcon aria-hidden className={refreshing ? "animate-spin" : ""} /></button>
          {canCreate && !adding ? <button className={ui.button()} onClick={() => { create.reset(); setAdding(true); }}>New project</button> : null}
        </div>
      </div>
      {adding ? (
        <form
          className={ui.cx(ui.card, ui.cardBody, "mb-6 gap-4")}
          onSubmit={(event) => {
            event.preventDefault();
            create.mutate(formText(new FormData(event.currentTarget), "name"));
          }}
        >
          <label className={ui.field}>
            Project name
            <input className={ui.input} name="name" required maxLength={120} autoFocus disabled={create.isPending} />
          </label>
          {create.error ? <p role="alert" className="text-sm text-danger">{create.error.message}</p> : null}
          <div className="flex gap-3">
            <button className={ui.button("primary")} disabled={create.isPending}>
              {create.isPending ? "Creating…" : "Create project"}
            </button>
            <button type="button" className={ui.button()} onClick={() => { setAdding(false); }} disabled={create.isPending}>
              Cancel
            </button>
          </div>
        </form>
      ) : null}
      <div className="grid items-start gap-7 xl:grid-cols-[280px_minmax(0,1fr)]">
        <section aria-labelledby="workspace-projects-title" className="min-w-0 xl:col-start-2 xl:row-start-1">
          <label className="relative mb-6 block">
            <MagnifyingGlassIcon size={18} aria-hidden className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" />
            <span className="sr-only">Search projects</span>
            <input type="search" aria-label="Search projects" placeholder="Search projects…" value={search} onChange={(event) => { setSearch(event.target.value); }} className={ui.cx(ui.input, "pl-10")} />
          </label>
          <div className="mb-3 flex items-baseline gap-2">
            <h2 id="workspace-projects-title" className={ui.sectionTitle}>Projects</h2>
            <span className="text-xs text-muted">{projects.data.length}</span>
          </div>
          {!projects.data.length ? <Empty title="No projects yet">{canCreate ? "Create a project to start adding resources." : "Your provider adds projects to this workspace for you."}</Empty> : !filtered.length ? <Empty title="No matching projects">Try another project name.</Empty> : (
            <ul aria-label="Projects" className="grid gap-4 sm:grid-cols-2 2xl:grid-cols-3">
              {filtered.map((project) => {
                const resources = workspace.plugins.flatMap((plugin) => plugin.resources.filter((resource) => resource.project.id === project.id).map((resource) => ({ ...resource, plugin: plugin.plugin })));
                const pending = workspace.pendingProjects.has(project.id), unavailable = workspace.failedProjects.has(project.id);
                return <li key={project.id} className="min-w-0">
                  <article aria-labelledby={`workspace-project-${project.id}`} className={ui.cx(ui.card, ui.clickableRow, "flex h-full min-h-44 flex-col p-5")}>
                    <div className="mb-5 flex min-w-0 items-center gap-3">
                      <span className="grid size-9 shrink-0 place-items-center rounded-md border border-outline bg-soft"><FolderSimpleIcon size={20} aria-hidden /></span>
                      <h3 id={`workspace-project-${project.id}`} className="min-w-0 truncate text-sm font-semibold" title={project.name}>
                        <Link className={ui.rowLink} to="/workspaces/$customer/projects/$id" params={{ customer: id, id: project.id }} onMouseEnter={() => { intent.hover(project.id); }} onMouseLeave={intent.cancel} onFocus={() => { intent.focus(project.id); }} onBlur={intent.cancel}>{project.name}</Link>
                      </h3>
                    </div>
                    {pending ? <p className="text-xs text-muted">Loading resources…</p> : unavailable ? <p className="text-xs text-muted">Resource details unavailable.</p> : resources.length ? (
                      <ul aria-label={`Resources in ${project.name}`} className="mb-4 grid gap-1 text-[13px] text-muted">{resources.slice(0, 2).map((resource) => <li key={`${resource.plugin}:${resource.id}`} className="truncate" title={resource.name}>{resource.name}</li>)}</ul>
                    ) : <p className="text-[13px] text-muted">No extension resources linked.</p>}
                    <div className="mt-auto flex items-center justify-between gap-3 border-t border-outline pt-3 text-xs text-muted">
                      <span>{pending || unavailable ? "Extension resources" : `${String(resources.length)} ${resources.length === 1 ? "extension resource" : "extension resources"}`}</span>
                      <span>Open project</span>
                    </div>
                  </article>
                </li>;
              })}
            </ul>
          )}
        </section>
        <aside aria-label="Workspace summary" className="grid min-w-0 gap-5 xl:col-start-1 xl:row-start-1">
          <WorkspaceUsageSummary customer={id} />
          <Attention customer={id} />
        </aside>
      </div>
    </section>
  );
}
