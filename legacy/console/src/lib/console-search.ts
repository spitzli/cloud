import { msgid } from '@webdock/i18n';
import { filterRows } from '@webdock/search';
import type { TenantSummary, TenantView, Site } from './studio-contracts';

export type ConsoleResult = { id: string; label: string; href: string; kind: 'navigation' | 'workspace' | 'website' | 'project' | 'application' | 'database'; description?: string; external?: boolean };
export type ConsoleScope = { customerID?: string; projectID?: string };
export type ConsoleSearchInput = ConsoleScope & { query: string };
export type ConsoleSearchResponse = { results: ConsoleResult[]; partial?: boolean; more?: boolean; error?: string };
export type ConsoleSearchSources = {
  session: () => Promise<{ operator: boolean; preview?: { status: string } } | null>;
  tenant: (id: string) => Promise<TenantView>;
  workspaces: () => Promise<TenantSummary[]>;
  sites: () => Promise<Site[]>;
  bridges: () => Promise<Record<string, unknown>>;
  project: (id: string) => Promise<{ customerID: string }>;
  projects: (customerID: string, query: string) => Promise<{ docs: { projectID: string; name: string }[]; totalDocs: number }>;
  allProjects: (query: string) => Promise<{ docs: { id: string | number; name: string }[]; totalDocs: number }>;
  apps: (id: string, query: string) => Promise<{ docs: { id: string; name: string; status: string }[]; totalDocs: number }>;
  databases: (id: string) => Promise<{ id: string; name: string; state: string }[]>;
  navigation: (scope: ConsoleScope, canManage: boolean, operator: boolean, preview: boolean) => ConsoleResult[];
};
const validID = (id: unknown) => id === undefined || (typeof id === 'string' && /^[1-9][0-9]{0,18}$/.test(id));
export async function queryConsole(input: ConsoleSearchInput, sources: ConsoleSearchSources): Promise<ConsoleSearchResponse> {
  if (!input || typeof input.query !== 'string' || input.query.length > 160 || !validID(input.customerID) || !validID(input.projectID)) return { results: [], error: msgid('Invalid search.') };
  const session = await sources.session();
  if (!session || session.preview?.status === 'expired') return { results: [], error: msgid('Sign in to search your workspace.') };
  // Authorize the selected scope before loading any scoped resource or shortcut.
  let customerID = input.customerID;
  let tenant: TenantView | undefined;
  try {
    if (input.projectID) {
      const project = await sources.project(input.projectID);
      if (customerID && project.customerID !== customerID) return { results: [], error: msgid('This project is not in the selected workspace.') };
      customerID = project.customerID;
    }
    if (customerID) tenant = await sources.tenant(customerID);
  } catch { return { results: [], error: msgid('This workspace is unavailable. Choose another workspace.') }; }
  const query = input.query.trim();
  const scope = { customerID, projectID: input.projectID };
  const navigation = filterRows(sources.navigation(scope, tenant?.canManage ?? false, session.operator, !!session.preview), query, item => `${item.label} ${item.description || ''}`);
  if (query.length < 2) return { results: navigation.slice(0, 8) };
  const tasks: Promise<{ results: ConsoleResult[]; more?: boolean }>[] = [];
  if (!customerID) tasks.push(sources.workspaces().then(rows => ({ results: filterRows(rows, query, row => `${row.name} ${row.id}`).map(row => ({ id: `workspace:${row.id}`, label: row.name, href: `/tenants/${row.id}`, kind: 'workspace' as const })) })));
  if (!input.projectID) tasks.push(Promise.all([tenant ? Promise.resolve(tenant.sites) : sources.sites(), sources.bridges()]).then(([sites, bridges]) => ({ results: filterRows(sites, query, site => `${site.name} ${site.url} ${site.id}`).flatMap(site => {
    let url: URL; try { url = new URL(site.url); } catch { return []; }
    if (!['https:', 'http:'].includes(url.protocol) || (session.preview && !bridges[site.id])) return [];
    return [{ id: `website:${site.id}`, label: site.name, description: url.hostname, href: bridges[site.id] ? `/sites/${site.id}` : site.url, external: !bridges[site.id], kind: 'website' as const }];
  }) })));
  if (customerID && !input.projectID) tasks.push(sources.projects(customerID, query).then(page => ({ results: page.docs.map(project => ({ id: `project:${project.projectID}`, label: project.name, href: session.operator ? `/projects/${project.projectID}` : `/hosting/projects/${project.projectID}`, kind: 'project' as const })), more: page.totalDocs > page.docs.length })));
  else if (!customerID && session.operator) tasks.push(sources.allProjects(query).then(page => ({ results: page.docs.map(project => ({ id: `project:${project.id}`, label: project.name, href: `/projects/${project.id}`, kind: 'project' as const })), more: page.totalDocs > page.docs.length })));
  if (input.projectID) {
    tasks.push(sources.apps(input.projectID, query).then(page => ({ results: page.docs.map(app => ({ id: `app:${app.id}`, label: app.name, href: `/hosting/apps/${app.id}`, kind: 'application' as const })), more: page.totalDocs > page.docs.length })));
    tasks.push(sources.databases(input.projectID).then(rows => ({ results: filterRows(rows, query, row => `${row.name} ${row.id}`).map(database => ({ id: `database:${database.id}`, label: database.name, href: `/hosting/databases/${database.id}`, kind: 'database' as const })) })));
  }
  const settled = await Promise.allSettled(tasks);
  const resources = settled.flatMap(result => result.status === 'fulfilled' ? result.value.results : []);
  const results = [...navigation, ...resources];
  return { results: results.slice(0, 40), more: results.length > 40 || settled.some(result => result.status === 'fulfilled' && result.value.more), partial: settled.some(result => result.status === 'rejected') };
}
