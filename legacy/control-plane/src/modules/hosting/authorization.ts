import { organizationPermission } from "../auth/clerk-provider";
import type { OrgPermission } from "../auth/organization-permissions";
import {
  currentConsolePrincipal,
  selectedIdentityDirectory,
} from "../auth/principal";
import { createHostingAuthorization } from "@webdock/module-hosting/server/authorization";
import { currentDatabaseScope } from "@webdock/database-runtime";
export type { Connection } from "@webdock/module-hosting/server/authorization";
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
