import { Link } from "@tanstack/react-router";
import { useI18n } from "@webdock/i18n/react";
import { useWorkspacePlugins, databasePlugin, pluginNav } from "./plugins/workspace";
import { Empty, Failure, Loading } from "./common";
import * as ui from "./ui";

export function WorkspaceDatabases({ customer }: { customer: string }) {
  const { t } = useI18n(), workspace = useWorkspacePlugins(customer);
  const databases = workspace.plugins.filter(p => databasePlugin(p.plugin) && (p.features.length || p.resources.length));
  return <section>
    <header className={ui.pageHeading}><div><h1 className={ui.pageTitle}>{t("Databases")}</h1><p className={ui.pageLead}>{t("Your database connections, organized by provider and project.")}</p></div></header>
    {workspace.pending ? <Loading>{t("Loading databases…")}</Loading>
      : workspace.error || workspace.incomplete ? <Failure error={workspace.error ?? new Error(t("Databases are unavailable."))} retry={() => void workspace.refetch()} />
      : !databases.length ? <Empty title={t("No databases yet")}>{t("Database connections added to your projects will appear here.")}</Empty>
      : <div className="grid gap-5">{databases.map(({ plugin, resources }) => {
        const { label: productLabel, Icon } = pluginNav(plugin);
        const label = plugin === "postgres" ? "PostgreSQL" : productLabel;
        return <section className={ui.card} key={plugin} aria-label={label}>
          <header className="flex items-center justify-between gap-3 px-5 py-4"><h2 className={ui.cx(ui.sectionTitle, "flex items-center gap-2")}><Icon size={20} />{label}</h2><Link className={ui.button()} to="/workspaces/$customer/products/$plugin" params={{ customer, plugin }}>{label}</Link></header>
          {resources.length ? <ul>{resources.map(resource => <li className={ui.cardRow} key={`${resource.project.id}:${resource.id}`}>
            <Link className="font-medium" to="/workspaces/$customer/projects/$id/resources/$plugin/$resource" params={{ customer, id: resource.project.id, plugin, resource: resource.id }}>{resource.name}</Link>
            <Link className="text-xs text-muted" to="/workspaces/$customer/projects/$id" params={{ customer, id: resource.project.id }}>{resource.project.name}</Link>
          </li>)}</ul> : <p className="px-5 pb-5 text-sm text-muted">{t("Open this provider to connect a database to a project.")}</p>}
        </section>;
      })}</div>}
  </section>;
}
