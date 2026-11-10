import { Link, useRouterState } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { tenantsSchema } from "./workspace-contracts";
import { useI18n } from "@webdock/i18n/react";
import { DatabaseIcon } from "@phosphor-icons/react/dist/csr/Database";
import { HouseIcon } from "@phosphor-icons/react/dist/csr/House";
import { UsersIcon } from "@phosphor-icons/react/dist/csr/Users";
import { GaugeIcon } from "@phosphor-icons/react/dist/csr/Gauge";
import { GlobeIcon } from "@phosphor-icons/react/dist/csr/Globe";
import { KeyIcon } from "@phosphor-icons/react/dist/csr/Key";
import { useApi, useSession } from "./context";
import { databasePlugin, pluginNav, useWorkspacePlugins, useWorkspaceProjects } from "./plugins/workspace";


export function useTenants() {
  const api = useApi();
  return useQuery({ queryKey: ["tenants"], queryFn: ({ signal }) => api.get("/api/console/tenants", tenantsSchema, signal) });
}
/** The workspace a page belongs to, from its address. */
export function workspaceOf(pathname: string) {
  return /^\/workspaces\/([1-9][0-9]{0,18})(?:\/|$)/.exec(pathname)?.[1];
}

function section(pathname: string): string {
  const plugin = /\/(?:products|resources)\/([a-z][a-z0-9-]*)/.exec(pathname)?.[1];
  if (plugin) return databasePlugin(plugin) ? "Databases" : pluginNav(plugin).label;
  if (pathname.endsWith("/databases")) return "Databases";
  if (pathname.includes("/projects/")) return "Projects";
  if (pathname.startsWith("/tenants/")) return "Members";
  if (pathname.endsWith("/usage")) return "Plan and usage";
  if (pathname.endsWith("/domains")) return "Domains";
  if (pathname.endsWith("/connections")) return "Connections";
  if (pathname.endsWith("/api-tokens")) return "API tokens";
  return "Overview";
}
/** Inside a customer workspace the header names it and the section, like the sidebar does. */
export function useWorkspaceHeader(pathname: string) {
  const { t } = useI18n();
  const tenants = useTenants();
  const customer = workspaceOf(pathname) ?? /^\/tenants\/([1-9][0-9]{0,18})$/.exec(pathname)?.[1];
  const projects = useWorkspaceProjects(customer);
  if (!customer) return null;
  const tenant = tenants.data?.find((t) => t.id === customer);
  const name = tenant?.name ?? "Workspace";
  const projectID = /^\/workspaces\/[^/]+\/projects\/([^/]+)/.exec(pathname)?.[1];
  const project = projects.data?.find(project => project.id === projectID);
  const currentSection = pathname.endsWith("/connections") ? t("Connections") : section(pathname) === "Databases" ? t("Databases") : section(pathname);
  const crumbs = project ? [name, project.name, ...(pathname.includes("/resources/") ? [currentSection] : [])] : [name, currentSection];
  return { customer, project, icon: tenant?.icon, label: name, detail: tenant?.role ?? "Customer workspace", crumbs };
}

/** Sidebar of one workspace: overview, one entry per product the viewer can see, members. */
export function WorkspaceNav({ customer, item }: { customer: string; item: (active: boolean) => string }) {
  const { t } = useI18n();
  const session = useSession();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const tenants = useTenants();
  const workspace = useWorkspacePlugins(customer);
  const tenant = tenants.data?.find((t) => t.id === customer);
  const manager = !(session.user.preview && !session.user.acting) && (session.user.role === "operator" || tenant?.role === "owner" || tenant?.role === "admin");
  const visible = workspace.plugins.filter((p) => p.features.length || p.resources.length);
  const databases = visible.filter(p => databasePlugin(p.plugin));
  const databaseActive = pathname.endsWith("/databases") || databases.some(p => pathname.startsWith(`/workspaces/${customer}/products/${p.plugin}`) || pathname.includes(`/resources/${p.plugin}/`));
  const overview = pathname === `/workspaces/${customer}` || pathname.startsWith(`/workspaces/${customer}/projects/`) && !pathname.includes("/resources/");
  return (
    <div className="mb-[18px]">
      <p className="truncate px-2.5 py-1.5 text-xs text-muted">{tenant?.name ?? "Workspace"}</p>
      <Link to="/workspaces/$id" params={{ id: customer }} className={item(overview)} aria-current={overview ? "page" : undefined}>
        <HouseIcon size={18} />
        Overview
      </Link>
      {databases.length ? <Link to="/workspaces/$id/databases" params={{ id: customer }} className={item(databaseActive)} aria-current={databaseActive ? "page" : undefined}>
        <DatabaseIcon size={18} /><span className="flex-1">{t("Databases")}</span><span className="text-xs text-muted">{databases.reduce((sum, p) => sum + p.resources.length, 0)}</span>
      </Link> : null}
      {visible.filter(p => !databasePlugin(p.plugin)).map(({ plugin, resources }) => {
        const { label, Icon } = pluginNav(plugin);
        const active = pathname.startsWith(`/workspaces/${customer}/products/${plugin}`) || pathname.includes(`/resources/${plugin}/`);
        return (
          <Link key={plugin} to="/workspaces/$customer/products/$plugin" params={{ customer, plugin }} className={item(active)} aria-current={active ? "page" : undefined}>
            <Icon size={18} />
            <span className="flex-1">{label}</span>
            {resources.length ? <span className="text-xs text-muted">{resources.length}</span> : null}
          </Link>
        );
      })}
      <Link to="/workspaces/$id/domains" params={{ id: customer }} className={item(pathname === `/workspaces/${customer}/domains`)} aria-current={pathname === `/workspaces/${customer}/domains` ? "page" : undefined}>
        <GlobeIcon size={18} />
        Domains
      </Link>
      <Link to="/workspaces/$id/connections" params={{ id: customer }} className={item(pathname === `/workspaces/${customer}/connections`)} aria-current={pathname === `/workspaces/${customer}/connections` ? "page" : undefined}>
        <KeyIcon size={18} />
        {t("Connections")}
      </Link>
      <Link to="/workspaces/$id/usage" params={{ id: customer }} className={item(pathname === `/workspaces/${customer}/usage`)} aria-current={pathname === `/workspaces/${customer}/usage` ? "page" : undefined}>
        <GaugeIcon size={18} />
        Plan and usage
      </Link>
      {manager && !session.user.preview ? (
        <Link to="/workspaces/$id/api-tokens" params={{ id: customer }} className={item(pathname === `/workspaces/${customer}/api-tokens`)} aria-current={pathname === `/workspaces/${customer}/api-tokens` ? "page" : undefined}>
          <KeyIcon size={18} />
          API tokens
        </Link>
      ) : null}
      {manager ? (
        <Link to="/$collection/$id" params={{ collection: "tenants", id: customer }} className={item(pathname === `/tenants/${customer}`)} aria-current={pathname === `/tenants/${customer}` ? "page" : undefined}>
          <UsersIcon size={18} />
          Members
        </Link>
      ) : null}
    </div>
  );
}
