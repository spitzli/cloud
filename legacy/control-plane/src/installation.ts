import {
  moduleForOperation,
  isModuleProvisioning,
} from "@webdock/module-sdk/selection";
import { createRegistry, type ModuleView, type ModuleManifest } from "@webdock/module-sdk";
export let builtinManifests: readonly ModuleManifest[] = [];
let mailProvisioning = isModuleProvisioning;
/** Trusted distribution composition, called before serving requests. */
export function configureInstallation(manifests: readonly ModuleManifest[], provisioning = isModuleProvisioning) {
  builtinManifests = manifests;
  mailProvisioning = provisioning;
}
export function installationRegistry(
  env: Record<string, string | undefined> = process.env,
) {
  let config: unknown;
  if (env.WEBDOCK_MODULES !== undefined) {
    try {
      config = JSON.parse(env.WEBDOCK_MODULES);
    } catch {
      throw Error("WEBDOCK_MODULES must contain valid installation JSON");
    }
  }
  return createRegistry(
    builtinManifests,
    config,
  );
}
export function requireMail(intent: "access" | "provision" = "access") {
  installationRegistry().require("mail", intent);
}
export function requireInstalledOperation(operation: string, args: unknown[]) {
  const module = moduleForOperation(operation, args);
  if (
    module === "mail" &&
    (operation === "hosting.command" || operation === "database.command")
  ) {
    const action = (args[0] as { action?: string })?.action;
    requireMail(
      action === "byok.mail.domain" || action === "byok.mail.connect"
        ? "provision"
        : "access",
    );
    return;
  }
  if (module)
    installationRegistry().require(
      module,
      (
        module === "mail"
          ? mailProvisioning(operation, args)
          : isModuleProvisioning(operation, args)
      )
        ? "provision"
        : "access",
    );
}
export function publicInstallation(
  env: Record<string, string | undefined> = process.env,
): ModuleView {
  const view = installationRegistry(env).publicView();
  if (env.WEBDOCK_API_DOMAINS !== undefined) {
    const domains = env.WEBDOCK_API_DOMAINS.split(",");
    for (const [module, domain] of Object.entries({
      mail: "mail",
      hosting: "hosting",
      databases: "databases",
      storage: "portal",
    })) {
      if (Object.hasOwn(view.modules, module) && !domains.includes(domain)) {
        view.modules[module] = "disabled";
        view.revision += `:${module}-unowned`;
      }
    }
  }
  return view;
}
export function mailRequest(path: string): boolean {
  return /^\/(?:api\/mail(?:\/|$)|mail\/docs(?:\/|$)|api\/cron\/mail(?:\/|$))/.test(
    path,
  );
}
export function validateWorkerModules(
  domains: string[],
  env: Record<string, string | undefined> = process.env,
) {
  const registry = installationRegistry(env);
  for (const domain of domains) registry.require(domain);
}

export function requireHostingCommand(action: string) {
  if (action.startsWith("byok.mail."))
    requireMail(
      action === "byok.mail.domain" || action === "byok.mail.connect"
        ? "provision"
        : "access",
    );
  else requireInstalledOperation("hosting.command", [{ action }]);
}
export function moduleForRequest(path: string) {
  if (mailRequest(path)) return "mail";
  if (/^\/api\/hosting\/agent(?:\/|$)/.test(path)) return undefined;
  if (/^\/api\/databases(?:\/|$)/.test(path)) return "databases";
  if (path === "/api/github/select" || /^\/api\/(?:hosting|vercel)(?:\/|$)/.test(path)) return "hosting";
  if (/^\/api\/cms(?:\/|$)/.test(path)) return "cms";
  return undefined;
}
