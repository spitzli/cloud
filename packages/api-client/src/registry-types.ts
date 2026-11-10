// Public DTO compatibility exports; no database/runtime dependency enters browser bundles.
export type {
  User,
  Customer,
  Project,
  AuditEvent,
  RegistryWhere,
} from "@spitzli/core/registry/types";
export type { CmsInstance } from "@spitzli/module-cms/server/registry-types";
