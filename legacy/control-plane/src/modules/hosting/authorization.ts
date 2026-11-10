import { organizationPermission } from "../auth/clerk-provider";
import type { OrgPermission } from "../auth/organization-permissions";
import {
  currentConsolePrincipal,
  selectedIdentityDirectory,
} from "../auth/principal";
import { createHostingAuthorization } from "@spitzli/module-hosting/server/authorization";
import { currentDatabaseScope } from "@spitzli/database-runtime";
export type { Connection } from "@spitzli/module-hosting/server/authorization";
export const { authorizeHosting } = createHostingAuthorization({
  identity: selectedIdentityDirectory,
  currentConsolePrincipal,
  organizationPermission: (subject, organizationID, permission) =>
    organizationPermission(
      subject,
      organizationID,
      permission as OrgPermission,
    ),
  get currentDatabaseScope() {
    return currentDatabaseScope;
  },
});
