import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { PlusIcon } from "@phosphor-icons/react/dist/csr/Plus";
import { useSession } from "../context";
import { isReadOnlyPreview } from "../api";
import { Empty, Failure, Loading } from "../common";
import { ChosenCreate } from "../ProjectPage";
import { creatable } from "./views";
import { pluginNav, useWorkspacePlugins } from "./workspace";
import { Offerings } from "./Offerings";
import * as ui from "../ui";

/** One product of a workspace, e.g. Databases: everything of that plugin across the workspace's
 *  projects, and the things this viewer may create. */
export function ProductPage({ customer, plugin }: { customer: string; plugin: string }) {
  const session = useSession();
  const workspace = useWorkspacePlugins(customer);
  const [adding, setAdding] = useState<string>();
  const [project, setProject] = useState<string>();
  const { label, Icon } = pluginNav(plugin);
  if (workspace.pending) return <Loading>Loading {label.toLowerCase()}…</Loading>;
  if (workspace.error || workspace.incomplete) return <Failure error={workspace.error ?? new Error("Resources are unavailable.")} retry={() => void workspace.refetch()} />;
  const projects = workspace.projects.data ?? [];
  const entry = workspace.plugins.find((p) => p.plugin === plugin);
  const operator = session.user.role === "operator" && !session.user.preview;
  const options = entry && !isReadOnlyPreview(session) ? [...new Map(entry.sources.flatMap(({ view }) => creatable(view).map((option) => [option.action.name, option] as const))).values()] : [];
  const chosen = options.find((o) => o.action.name === adding);
  const eligible = entry?.sources.filter(({ view }) => creatable(view).some((option) => option.action.name === adding)) ?? [];
  const target = eligible.find((source) => source.project.id === project);
  const targetOption = target && creatable(target.view).find((option) => option.action.name === adding);
  const choose = (action: string) => {
    setProject(entry?.sources.find(({ view }) => creatable(view).some((option) => option.action.name === action))?.project.id);
    setAdding(action);
  };
  const offerings = entry?.view.manifest.offerings ?? [];
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.cx(ui.pageTitle, "flex items-center gap-2.5")}><Icon size={26} />{label}</h1>
          <p className={ui.pageLead}>{entry?.view.manifest.description ?? "Not available in this workspace."}</p>
        </div>
        {options.length && projects.length ? (
          <details className="relative">
            <summary className={ui.cx(ui.button("primary"), "list-none [&::-webkit-details-marker]:hidden")}>
              <PlusIcon />
              {operator ? "Assign" : "Create"}
            </summary>
            <ul className={ui.actionMenu} aria-label="Create">
              {options.map((option) => (
                <li key={option.action.name}>
                  <button
                    type="button"
                    className="w-full rounded-md px-3 py-2 text-left text-sm hover:bg-subtle"
                    onClick={(event) => {
                      event.currentTarget.closest("details")?.removeAttribute("open");
                      choose(option.action.name);
                    }}
                  >
                    <span className="block font-medium">{option.offering.label}</span>
                    {option.offering.description ? <span className="mt-0.5 line-clamp-2 text-[13px] text-muted">{option.offering.description}</span> : null}
                  </button>
                </li>
              ))}
            </ul>
          </details>
        ) : null}
      </div>
      {chosen && entry ? (
        <div className="mb-6 grid gap-3">
          {chosen.offering.description ? <p className={ui.notice}>{chosen.offering.description}</p> : null}
          {eligible.length > 1 || !target ? (
            <label className={ui.cx(ui.field, "max-w-sm")}>
              Project
              <select className={ui.input} value={target?.project.id ?? ""} onChange={(event) => { setProject(event.target.value); }}>
                {!target ? <option value="" disabled>Choose a project</option> : null}
                {eligible.map(({ project: p }) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            </label>
          ) : <div role="group" aria-label="Project destination" className={ui.field}><span>Project</span><span className="font-normal">{target.project.name}</span></div>}
          {target && targetOption ? <ChosenCreate
            key={`${chosen.action.name}:${target.project.id}`}
            view={target.view}
            action={targetOption.action}
            title={targetOption.offering.label}
            customer={customer}
            projectID={target.project.id}
            onCancel={() => { setAdding(undefined); setProject(undefined); }}
          /> : <p role="status" className={ui.notice}>Choose an available project to continue.</p>}
        </div>
      ) : null}
      {!projects.length ? (
        <Empty title={`No ${label.toLowerCase()} yet`}>Your provider sets this workspace up for you.</Empty>
      ) : !entry?.resources.length ? (
        !chosen ? (
          <div className="grid gap-4">
            <p className="text-sm text-muted">
              {options.length ? `No ${label.toLowerCase()} yet. Pick what fits:` : `No ${label.toLowerCase()} yet. Your provider adds these for you; this is what they can be:`}
            </p>
            <Offerings offerings={offerings} create={(id) => {
              const option = options.find((o) => o.offering.id === id);
              return option ? () => { choose(option.action.name); } : undefined;
            }} />
          </div>
        ) : null
      ) : (
        <div className={ui.tableFrame}>
          <table className={ui.table}>
            <thead>
              <tr>
                <th className={ui.th}>Name</th>
                <th className={ui.th}>Type</th>
                {projects.length > 1 ? <th className={ui.th}>Project</th> : null}
              </tr>
            </thead>
            <tbody>
              {entry.resources.map((resource) => (
                <tr key={`${resource.project.id}:${resource.id}`} className={ui.tr}>
                  <td className={ui.td}>
                    <Link
                      className={ui.cx(ui.rowLink, "font-medium")}
                      to="/workspaces/$customer/projects/$id/resources/$plugin/$resource"
                      params={{ customer, id: resource.project.id, plugin, resource: resource.id }}
                    >
                      {resource.name}
                    </Link>
                    {resource.createdVia === "operator" ? <span className="ml-2 text-[13px] text-muted">added by your provider</span> : null}
                  </td>
                  <td className={ui.td}>{offerings.find((o) => o.id === resource.offering)?.label ?? resource.offering}</td>
                  {projects.length > 1 ? <td className={ui.td}>{resource.project.name}</td> : null}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {entry?.resources.length && offerings.some((o) => o.description) ? (
        <details className="mt-8">
          <summary className="cursor-pointer text-sm font-medium">Which one do I need?</summary>
          <div className="mt-4"><Offerings offerings={offerings} /></div>
        </details>
      ) : null}
    </section>
  );
}
