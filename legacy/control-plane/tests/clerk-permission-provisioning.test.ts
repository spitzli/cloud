import { describe, expect, test } from 'bun:test';
import { APPLICATION_ID, INSTANCES, PERMISSIONS, configure, type Provider, type Target } from '../scripts/configure-clerk-permissions';
const target: Target = { app: APPLICATION_ID, instance: INSTANCES.dev, environment: 'dev' };
function fixture() {
  const system = { id: 'sys', key: 'org:sys_memberships:manage' };
  const unrelated = { id: 'unrelated', key: 'org:mail:manage' };
  const permissions = [system, unrelated];
  const roles = [
    { id: 'admin', key: 'org:admin', permissions: [system, unrelated] },
    { id: 'member', key: 'org:member', permissions: [] as { id: string; key: string }[] },
  ];
  const mutations: string[] = [];
  let instance = INSTANCES.dev as string;
  const provider: Provider = {
    async request<T>(path: string, options?: { platform?: boolean; body?: unknown }) {
      let result: unknown;
      if (options?.platform) result = { application_id: APPLICATION_ID, instances: [{ instance_id: instance, environment_type: 'development' }] };
      else if (options?.body !== undefined) {
        mutations.push(path);
        if (path === '/organization_permissions') {
          const body = options.body as { key: string };
          result = { id: `perm${permissions.length}`, key: body.key };
          permissions.push(result as { id: string; key: string });
        } else {
          const id = path.split('/').at(-1);
          roles[0]!.permissions.push(permissions.find(p => p.id === id)!);
          result = roles[0];
        }
      } else {
        let data: unknown[] = [];
        if (path.startsWith('/organization_roles?')) data = roles;
        if (path.startsWith('/organization_permissions?')) data = permissions;
        if (path.startsWith('/role_sets?')) data = [{ id: 'set', key: 'role_set:default', roles, role_set_migration: null }];
        if (path.startsWith('/organizations?')) data = [{ id: 'org1' }];
        if (path.includes('/memberships?')) data = roles.map(r => ({ role: r.key, permissions: r.permissions.map(p => p.key) }));
        result = { data, total_count: data.length };
      }
      return structuredClone(result) as T;
    },
  };
  return { provider, roles, mutations, wrongInstance: () => { instance = INSTANCES.prod; } };
}
describe('Clerk permission provisioning', () => {
  test('default dry run never writes', async () => {
    const f = fixture();
    expect((await configure(f.provider, target)).actions).toHaveLength(6);
    expect(f.mutations).toEqual([]);
    expect(PERMISSIONS.some(permission => (permission.key as string) === 'org:billing:manage')).toBe(false);
  });
  test('additive apply retains system/unrelated permissions and repeat is a no-op', async () => {
    const f = fixture();
    expect((await configure(f.provider, target, true)).verifiedMemberships).toBe(2);
    expect(f.roles[0]!.permissions.map(p => p.key)).toEqual(['org:sys_memberships:manage', 'org:mail:manage', ...PERMISSIONS.map(p => p.key)]);
    expect(f.roles[1]!.permissions).toEqual([]);
    expect((await configure(f.provider, target, true)).actions).toEqual([]);
    expect(f.mutations).toHaveLength(6);
  });
  test('wrong explicit target and live instance fail before writes', async () => {
    const f = fixture();
    await expect(configure(f.provider, { ...target, instance: INSTANCES.prod }, true)).rejects.toThrow('unexpected');
    f.wrongInstance();
    await expect(configure(f.provider, target, true)).rejects.toThrow('identity mismatch');
    expect(f.mutations).toEqual([]);
  });
  test('unexpected Member grant stops without automatic revocation', async () => {
    const f = fixture();
    f.roles[1]!.permissions.push({ id: 'unexpected', key: PERMISSIONS[0].key });
    await expect(configure(f.provider, target, true)).rejects.toThrow('manual review');
    expect(f.mutations).toEqual([]);
    expect(f.roles[1]!.permissions).toHaveLength(1);
  });
});
