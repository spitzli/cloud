/** CLI-backed provisioning; no secrets or env files are read by this script. */
import { spawnSync } from 'node:child_process';
import { ORGANIZATION_PERMISSIONS } from '../src/modules/auth/organization-permissions';

export const APPLICATION_ID = 'app_3KjGKUGb7dYUMgkjKC0eCWghnOB';
export const INSTANCES = { dev: 'ins_3KjGKQnAATh2cA4ddJBR2cg3NXu', prod: 'ins_3Klz8x6R4XYa8w6QfjPQl95EKZf' } as const;
export const PERMISSIONS = [
  { key: 'org:projects:manage', name: 'Manage projects', description: 'Manage Webdock self-service projects and applications.' },
  { key: 'org:deployments:create', name: 'Create deployments', description: 'Build, publish, retry and roll back Webdock deployments.' },
  { key: 'org:databases:manage', name: 'Manage databases', description: 'Manage Webdock databases and database access.' },
] as const;
type Permission = { id: string; key: string };
type Role = Permission & { permissions: Permission[] };
type RoleSet = Permission & { roles: Permission[]; role_set_migration: unknown };
type Membership = { role: string; permissions: string[] };
export type Target = { app: string; instance: string; environment: 'dev' | 'prod' };
export interface Provider {
  request<T>(path: string, options?: { platform?: boolean; body?: unknown }): Promise<T>;
}

function assertTarget(target: Target) {
  if (target.app !== APPLICATION_ID || target.instance !== INSTANCES[target.environment]) {
    throw new Error('Refusing unexpected Webdock application, environment or instance');
  }
}
async function list<T>(provider: Provider, path: string): Promise<T[]> {
  const items: T[] = [];
  for (;;) {
    const page = await provider.request<{ data: T[]; total_count: number }>(`${path}?limit=100&offset=${items.length}`);
    if (!Array.isArray(page.data) || !Number.isInteger(page.total_count)) throw new Error(`Malformed list: ${path}`);
    items.push(...page.data);
    if (items.length >= page.total_count) return items;
    if (!page.data.length) throw new Error(`Incomplete list: ${path}`);
  }
}
function baseline(roles: Role[], sets: RoleSet[]) {
  const admins = roles.filter(r => r.key === 'org:admin');
  const members = roles.filter(r => r.key === 'org:member');
  if (admins.length !== 1 || members.length !== 1) throw new Error('Expected exactly one existing Admin and Member');
  for (const role of roles) {
    if (!Array.isArray(role.permissions)) throw new Error('Missing role permissions');
    if (role.key !== 'org:admin' && role.permissions.some(p => PERMISSIONS.some(w => w.key === p.key))) {
      throw new Error(`Unexpected Webdock permission on ${role.key}; manual review required, no automatic revocation`);
    }
  }
  if (!sets.length || sets.some(s => s.role_set_migration || !s.roles.some(r => r.id === admins[0]!.id) || !s.roles.some(r => r.id === members[0]!.id))) {
    throw new Error('Unexpected role set or active role-set migration; manual review required');
  }
  return { admin: admins[0]!, member: members[0]! };
}
export async function configure(provider: Provider, target: Target, apply = false) {
  assertTarget(target);
  if (JSON.stringify(PERMISSIONS.map(p => p.key).sort()) !== JSON.stringify(ORGANIZATION_PERMISSIONS.filter(key => key !== 'org:billing:manage').sort())) throw new Error('Provisioning catalog differs from application permissions');
  const app = await provider.request<{ application_id: string; instances: { instance_id: string; environment_type: string }[] }>(`/platform/applications/${target.app}`, { platform: true });
  if (app.application_id !== target.app || !app.instances.some(i => i.instance_id === target.instance && i.environment_type === (target.environment === 'dev' ? 'development' : 'production'))) {
    throw new Error('Live application/instance identity mismatch');
  }
  const roles = await list<Role>(provider, '/organization_roles');
  const sets = await list<RoleSet>(provider, '/role_sets');
  const { admin } = baseline(roles, sets);
  const permissions = await list<Permission>(provider, '/organization_permissions');
  for (const desired of PERMISSIONS) {
    if (permissions.filter(p => p.key === desired.key).length > 1) throw new Error(`Duplicate permission ${desired.key}`);
  }
  const actions: string[] = [];
  for (const desired of PERMISSIONS) {
    const matches = permissions.filter(p => p.key === desired.key);
    if (matches.length > 1) throw new Error(`Duplicate permission ${desired.key}`);
    let permission = matches[0];
    if (!permission) {
      actions.push(`create ${desired.key}`);
      if (apply) permission = await provider.request<Permission>('/organization_permissions', { body: desired });
    }
    if (!admin.permissions.some(p => p.key === desired.key)) {
      actions.push(`attach ${desired.key} to org:admin`);
      if (apply) {
        if (!permission?.id || permission.key !== desired.key) throw new Error('Unexpected created permission');
        await provider.request(`/organization_roles/${encodeURIComponent(admin.id)}/permissions/${encodeURIComponent(permission.id)}`, { body: {} });
      }
    }
  }
  let verifiedMemberships = 0;
  if (apply) {
    const after = await list<Role>(provider, '/organization_roles');
    const { admin: updated } = baseline(after, await list<RoleSet>(provider, '/role_sets'));
    for (const before of roles) {
      const current = after.find(r => r.id === before.id);
      if (!current || before.permissions.some(p => !current.permissions.some(q => q.id === p.id))) throw new Error('Existing role permissions changed');
      if (before.key !== 'org:admin' && JSON.stringify(before.permissions.map(p => p.id).sort()) !== JSON.stringify(current.permissions.map(p => p.id).sort())) throw new Error('Non-Admin role changed');
    }
    if (PERMISSIONS.some(p => !updated.permissions.some(q => q.key === p.key))) throw new Error('Admin permission readback failed');
    for (const org of await list<{ id: string }>(provider, '/organizations')) {
      for (const membership of await list<Membership>(provider, `/organizations/${encodeURIComponent(org.id)}/memberships`)) {
        if (!Array.isArray(membership.permissions)) throw new Error('Membership permissions missing');
        const granted = PERMISSIONS.filter(p => membership.permissions.includes(p.key));
        if ((membership.role === 'org:admin' && granted.length !== PERMISSIONS.length) || (membership.role !== 'org:admin' && granted.length)) throw new Error('Live membership permission readback failed');
        verifiedMemberships++;
      }
    }
  }
  return { environment: target.environment, instance: target.instance, apply, actions, verifiedMemberships };
}

export function cliProvider(target: Target): Provider {
  return {
    async request<T>(path: string, options?: { platform?: boolean; body?: unknown }) {
      const args = ['clerk@latest', 'api', path, '--app', target.app, '--instance', target.instance];
      if (options?.platform) args.push('--platform');
      if (options?.body !== undefined) args.push('-X', 'POST', '-d', JSON.stringify(options.body));
      const run = (extra: string[]) => {
        const result = spawnSync('bunx', [...args, ...extra], { encoding: 'utf8', maxBuffer: 10 * 1024 * 1024 });
        if (result.status !== 0) throw new Error(`Clerk CLI failed for ${path}; inspect with the authenticated CLI (no automatic plan upgrade)`);
        return result.stdout;
      };
      if (options?.body !== undefined) run(['--dry-run']);
      const output = run(options?.body !== undefined ? ['--yes'] : []);
      return (output.trim() ? JSON.parse(output) : null) as T;
    },
  };
}
if (import.meta.main) {
  try {
    const args = process.argv.slice(2);
    const allowed = new Set(['--app', '--instance', '--environment', '--apply']);
    for (let i = 0; i < args.length; i++) {
      if (!allowed.has(args[i]!)) throw new Error(`Unknown argument ${args[i]}`);
      if (args[i] !== '--apply') i++;
    }
    const value = (flag: string) => args.includes(flag) ? args[args.indexOf(flag) + 1] : undefined;
    const environment = value('--environment');
    if (environment !== 'dev' && environment !== 'prod') throw new Error('Explicit --environment dev|prod required');
    const target: Target = { app: value('--app')!, instance: value('--instance')!, environment };
    assertTarget(target);
    console.log(JSON.stringify(await configure(cliProvider(target), target, args.includes('--apply')), null, 2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : 'Provisioning failed');
    process.exitCode = 1;
  }
}
