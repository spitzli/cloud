import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { z } from "zod";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { useApi, useSession } from "./context";
import { Empty, Failure, Loading } from "./common";
import { formText } from "./form";
import * as ui from "./ui";
import { pluginNav, useWorkspacePlugins } from "./plugins/workspace";
import { Attention } from "./Notices";

const tenantsSchema = z.array(z.object({ id: z.string(), name: z.string(), status: z.string(), role: z.string().nullable() }));
const projectsSchema = z.array(z.object({ id: z.string(), name: z.string(), status: z.string() }));

/** A workspace's projects. Owners, admins and operators start new ones here. */
export function WorkspaceProjects({ id }: { id: string }) {
  const api = useApi();
  const session = useSession();
  const client = useQueryClient();
  const navigate = useNavigate();
  const tenants = useQuery({ queryKey: ["tenants"], queryFn: ({ signal }) => api.get("/api/console/tenants", tenantsSchema, signal) });
  const projects = useQuery({
    queryKey: ["workspace-projects", id],
    queryFn: ({ signal }) => api.get(`/api/console/tenant/${encodeURIComponent(id)}/projects`, projectsSchema, signal),
  });
  const workspace = useWorkspacePlugins(id);
  const [adding, setAdding] = useState(false);
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
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <Link className={ui.backLink} to="/$collection" params={{ collection: "tenants" }}>
            <ArrowLeftIcon />
            All workspaces
          </Link>
          <h1 className={ui.pageTitle}>{tenant?.name ?? "Workspace"}</h1>
          <p className={ui.pageLead}>Everything this workspace runs. Projects group resources that belong together.</p>
        </div>
        {canCreate && !adding && projects.data.length > 1 ? (
          <button className={ui.button()} onClick={() => { setAdding(true); }}>
            New project
          </button>
        ) : null}
      </div>
      <Attention customer={id} />
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
      {workspace.plugins.some((p) => p.features.length || p.resources.length) ? (
        <ul className="mb-8 grid gap-3 sm:grid-cols-2 lg:grid-cols-3" aria-label="Products">
          {workspace.plugins.filter((p) => p.features.length || p.resources.length).map(({ plugin, resources, view }) => {
            const { label, Icon } = pluginNav(plugin);
            const kinds = view.manifest.offerings.map((offering) => ({ label: offering.label, count: resources.filter((r) => r.offering === offering.id).length })).filter((k) => k.count);
            return (
              <li key={plugin} className={ui.cx(ui.card, ui.clickableRow, "relative grid gap-2 p-5")}>
                <Link className={ui.cx(ui.rowLink, "flex items-center gap-2 font-semibold")} to="/workspaces/$customer/products/$plugin" params={{ customer: id, plugin }}>
                  <Icon size={20} />
                  {label}
                </Link>
                <p className="text-[13px] text-muted">
                  {kinds.length ? kinds.map((k) => `${String(k.count)} ${k.label.toLowerCase()}`).join(", ") : "Nothing yet"}
                </p>
              </li>
            );
          })}
        </ul>
      ) : null}
      {projects.data.length <= 1 && !adding ? (
        <p className="text-[13px] text-muted">
          Everything lives in one project for now.
          {canCreate ? <> Need separate access for a team, or a staging environment? <button type="button" className="underline underline-offset-2" onClick={() => { setAdding(true); }}>Add a project</button>.</> : null}
        </p>
      ) : null}
      {projects.data.length > 1 ? <h2 className={ui.cx(ui.sectionTitle, "mb-3")}>Projects</h2> : null}
      {projects.data.length <= 1 ? null : projects.data.length === 0 ? (
        <Empty title="No projects yet">
          {canCreate ? "Create a project to start adding resources." : "Your provider adds projects to this workspace for you."}
        </Empty>
      ) : (
        <ul className={ui.card}>
          {projects.data.map((project) => (
            <li key={project.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
              <Link className={ui.cx(ui.rowLink, "font-medium")} to="/workspaces/$customer/projects/$id" params={{ customer: id, id: project.id }}>
                {project.name}
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
