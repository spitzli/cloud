import { LogHistory } from "./LogHistory";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon } from "@phosphor-icons/react/dist/csr/ArrowLeft";
import { useApi } from "../context";
import { Empty, Failure, Loading } from "../common";
import { parsePanel } from "./contracts";
import { PluginPanel } from "./Panel";
import { invokePlugin, usePluginViews, useProject } from "./views";
import { useNativeUI } from "./native";
import { pluginNav, useWorkspaceProjects } from "./workspace";
import * as ui from "../ui";

/** One plugin resource, rendered from the plugin's self-service resource page. */
export function ResourcePage({ customer, projectID, plugin, resource }: { customer: string; projectID: string; plugin: string; resource: string }) {
  const api = useApi();
  const client = useQueryClient();
  const navigate = useNavigate();
  const project = useProject(projectID);
  const views = usePluginViews(projectID);
  const projects = useWorkspaceProjects(customer);
  const view = views.data?.find((v) => v.manifest.name === plugin);
  const indexed = view?.resources.find((r) => r.id === resource);
  const page = view?.manifest.areas.selfService?.pages[0];
  const native = useNativeUI(view?.manifest.name);
  const Native = native.data?.Resource;
  const [replaced, setReplaced] = useState<unknown>();
  const context = { resource_id: resource };
  const panel = useQuery({
    queryKey: ["plugin-resource", plugin, projectID, resource, page?.action ?? ""],
    queryFn: ({ signal }) => invokePlugin(api, plugin, projectID, page?.action ?? "", context, signal),
    enabled: view?.state === "enabled" && view.runtime === "ready" && Boolean(page && indexed) && native.isSuccess && !Native,
    retry: false,
    // The plugin decides how often its page needs fresh state.
    refetchInterval: (query) => {
      try {
        return (parsePanel(query.state.data).refreshSeconds ?? 0) * 1000 || false;
      } catch {
        return false;
      }
    },
  });
  const back = (
    <Link className={ui.backLink} to="/workspaces/$customer/products/$plugin" params={{ customer, plugin }}>
      <ArrowLeftIcon />
      {pluginNav(plugin).label}
    </Link>
  );
  if (views.isPending || native.isLoading) return <Loading>Loading…</Loading>;
  if (views.error) return <Failure error={views.error} retry={() => void views.refetch()} />;
  const leave = async () => {
    await client.invalidateQueries({ queryKey: ["project-plugins"] });
    await navigate({ to: "/workspaces/$customer/products/$plugin", params: { customer, plugin } });
  };
  if (view && indexed && Native)
    return (
      <section>
        <div className={ui.pageHeading}>
          <div>
            {back}
            <h1 className={ui.pageTitle}>{indexed.name}</h1>
            <p className={ui.pageLead}>
              {view.manifest.offerings.find((o) => o.id === indexed.offering)?.label}
              {(projects.data?.length ?? 0) > 1 ? <> in <Link to="/workspaces/$customer/projects/$id" params={{ customer, id: projectID }}>{project.data?.name ?? "its project"}</Link></> : null}
            </p>
            {view.manifest.offerings.find((o) => o.id === indexed.offering)?.description ? (
              <p className="mt-1 max-w-[76ch] text-[13px] text-muted">{view.manifest.offerings.find((o) => o.id === indexed.offering)?.description}</p>
            ) : null}
          </div>
        </div>
        <Native
          plugin={plugin}
          projectID={projectID}
          customer={customer}
          resource={indexed}
          features={view.features}
          actions={view.manifest.actions.map(action => action.name)}
          writable={["owner", "admin", "operator"].includes(project.data?.role ?? "")}
          onDeleted={() => void leave()}
        />
        <LogHistory projectID={projectID} plugin={plugin} resource={indexed.id} viewer={project.data} />
      </section>
    );
  if (!view || !indexed || !page)
    return (
      <section>
        <div className={ui.pageHeading}><div>{back}</div></div>
        <Empty title="Not available">This resource no longer exists or you cannot open it.</Empty>
      </section>
    );
  const offering = view.manifest.offerings.find((o) => o.id === indexed.offering);
  const deletes = new Set(view.manifest.actions.filter((a) => a.resource === "delete").map((a) => a.name));
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          {back}
          <h1 className={ui.pageTitle}>{indexed.name}</h1>
          <p className={ui.pageLead}>{offering?.label ?? page.label}</p>
        </div>
      </div>
      {panel.isPending ? <Loading>Loading {page.label.toLowerCase()}…</Loading> : panel.error ? (
        <Failure error={panel.error} retry={() => void panel.refetch()} />
      ) : (
        <PluginPanel
          value={replaced ?? panel.data}
          context={context}
          invoke={async (action, input, signal) => {
            const result = await invokePlugin(api, plugin, projectID, action, input, signal);
            if (deletes.has(action)) {
              await leave();
              return result;
            }
            if (action === page.action) setReplaced(result);
            else void panel.refetch();
            return result;
          }}
        />
      )}
    </section>
  );
}
