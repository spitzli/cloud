import { authorizeHosting } from "./authorization";
import { createHostingPermissions } from "@webdock/module-hosting/server/permissions";
export const { authorizeSetupConfiguration } = createHostingPermissions({
  authorizeHosting,
});

export { gitActionPermission } from "@webdock/module-hosting/server/permissions";
