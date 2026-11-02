import { msgid } from '@webdock/i18n';

const within = (path: string, base: string) => path === base || path.startsWith(base + '/');
export function mainArea(path: string, operator: boolean): string | null {
  if (!operator) {
    if (within(path, '/sites')) return 'sites';
    if (within(path, '/tenants') || within(path, '/hosting')) return 'workspace';
    return null;
  }
  if (path === '/' || ['/projects', '/sites', '/hosting/projects', '/hosting/apps', '/hosting/databases'].some(base => within(path, base))) return 'projects';
  if (['/customers', '/tenants', '/hosting/clusters', '/hosting/vercel'].some(base => within(path, base))) return 'customers';
  if (['/operations', '/infrastructure', '/activity', '/hosting/operations'].some(base => within(path, base))) return 'operations';
  if (within(path, '/people')) return 'people';
  if (within(path, '/admin/plans')) return 'plans';
  if (within(path, '/integrations')) return 'connections';
  if (path === '/admin') return 'settings';
  return null;
}

export function areaSelected(href: string, pathname: string, search: string, exact = false, activePaths: string[] = []): boolean {
  if (activePaths.some(path => areaSelected(path, pathname, search))) return true;
  const [path, query] = href.split('?');
  if (!(exact || query ? pathname === path : within(pathname, path))) return false;
  if (!query) return true;
  const current = new URLSearchParams(search);
  return [...new URLSearchParams(query)].every(([key, value]) => (current.get(key) ?? (key === 'kind' ? 'products' : key === 'view' ? 'overview' : null)) === value);
}

export function soleWorkspace(tenants: {id: string; status: string}[], invitations: number, operator: boolean) {
  const active = tenants.filter(tenant => tenant.status === 'active');
  return !operator && !invitations && active.length === 1 ? `/tenants/${encodeURIComponent(active[0].id)}` : null;
}

export function applicationState(status: string) {
  return ({ draft: msgid('Awaiting first deployment'), running: msgid('Running'), ready: msgid('Ready'), stopped: msgid('Stopped'), pending: msgid('Change in progress'), failed: msgid('Failed'), error: msgid('Error'), deleted: msgid('Deleted'), 'needs-reconciliation': msgid('Review required') } as Record<string, string>)[status] ?? msgid('Unknown');
}



export { mailSendingPaths } from "@distribution/navigation";
