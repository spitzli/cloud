import { authorizeHosting } from "./authorization";
import { createHostingPermissions } from "@spitzli/module-hosting/server/permissions";
export const { authorizeSetupConfiguration } = createHostingPermissions({
  authorizeHosting,
});

export { gitActionPermission } from "@spitzli/module-hosting/server/permissions";
