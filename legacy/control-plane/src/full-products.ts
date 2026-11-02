import { configureMCPProducts } from "./modules/mcp/product-contributions";
import { projectVercelStatus } from "./modules/integrations/vercel/vercel-project";
import { configurePreviewProducts } from "./modules/access/tenant-preview";
import { readPreviewProduct } from "./modules/access/tenant-preview-products";
import { configureFullInstallation } from "./full-installation";
import { configureProductOperations } from "./modules/portal/product-operation";
import { configureStudioProducts } from "./modules/portal/studio-api";
import { mailOperationShapes } from "./modules/mail/studio-shapes";
import { PlanError, getPlans, getTenantPlan, managePlans, getOffer, acceptOffer } from "./modules/plans/plans";
import { HostingError } from "@webdock/hosting-contracts";
import { configureModulePreflight } from "./module-preflight";
import { verifyModuleShutdown } from "./full-module-preflight";
import { requireHostingCommand } from "./installation";
import { configureManagedApps, configureManagedAppProvisioning, configurePluginResourceBindings } from "./modules/plugins/runtime";
import { createManagedAppsHandler } from "./modules/hosting/plugin-apps";
import { executeHosting } from "./modules/hosting/service";
import { configureOperatorAppTemplates } from "./modules/hosting/approved-templates";
import { resolveApprovedTemplateHTTP, resolveApprovedTemplateApps } from "./modules/hosting/plugin-template-runtime";
export function configureFullProducts() {
  configureManagedApps(createManagedAppsHandler(executeHosting, requireHostingCommand));
  configurePluginResourceBindings({http:resolveApprovedTemplateHTTP,apps:resolveApprovedTemplateApps});
  const provisioning = configureOperatorAppTemplates();
  if(provisioning) configureManagedAppProvisioning(provisioning);
  configureMCPProducts(projectVercelStatus);
  configurePreviewProducts(readPreviewProduct);
  configureFullInstallation();
  configureModulePreflight(verifyModuleShutdown);
  configureProductOperations(async module => {
    switch (module) {
      case "mail": return import("./modules/mail/studio-operations");
      case "storage": return import("./modules/platform/storage-operations");
      case "hosting": case "databases": return import("./modules/portal/hosting-operations");
    }
  });
  const string = (v: unknown) => typeof v === "string" && v.length <= 32768;
  const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
  const input = (v: unknown) => object(v) && Object.keys(v).length <= 100 && Object.values(v).every(string);
  configureStudioProducts({
    shapes: { ...mailOperationShapes, "integrations.github": [input], "integrations.vercel": [string], "integrations.linkVercel": [input], "integrations.projectStatus": [string], "hosting.command": [object], "database.command": [object], getPlans: [], getTenantPlan: [string], managePlans: [input], getOffer: [string], acceptOffer: [string], getTenantStorage: [string], manageTenantStorage: [string, input] },
    async dispatch(operation, args, headers) {
      switch (operation) {
        case "getPlans": return getPlans(headers);
        case "getTenantPlan": return getTenantPlan(headers, args[0] as string);
        case "managePlans": return managePlans(headers, args[0] as Record<string,string>);
        case "getOffer": return getOffer(headers, args[0] as string);
        case "acceptOffer": return acceptOffer(headers, args[0] as string);
      }
      throw Error("Unsupported product operation");
    },
    publicError(error) {
      if (error instanceof HostingError) return { kind: "HostingError", message: error.message, status: error.status };
      if (error instanceof PlanError) return { kind: "PlanError", message: error.message, status: 400 };
    },
  });
}
