/** These custom keys are provisioned in Clerk; native team/system permissions stay separate. */
export const ORGANIZATION_PERMISSIONS = [
  "org:projects:manage",
  "org:deployments:create",
  "org:databases:manage",
  "org:billing:manage",
] as const;
export type OrgPermission = typeof ORGANIZATION_PERMISSIONS[number];
export type OrganizationPermission = OrgPermission;
export function normalizeOrganizationPermissions(value: unknown): OrgPermission[] {
  if (!Array.isArray(value)) return [];
  return ORGANIZATION_PERMISSIONS.filter(permission => value.includes(permission));
}
export function organizationCapabilities(permissions: readonly string[] | undefined) {
  const allowed = normalizeOrganizationPermissions(permissions);
  return {
    manageProjects: allowed.includes("org:projects:manage"),
    createDeployments: allowed.includes("org:deployments:create"),
    manageDatabases: allowed.includes("org:databases:manage"),
    manageBilling: allowed.includes("org:billing:manage"),
  };
}

/** Clerk role keys are organization-scoped; custom keys never imply global privileges. */
export function isClerkOrganizationRole(value: unknown): value is string {
  return typeof value === "string" && /^org:[a-z0-9_]+$/.test(value);
}
