import type { ModuleManifest } from "@spitzli/module-sdk";
export const mailManifest = {
  id: "mail",
  version: "0.1.0",
  sdkVersion: 1,
  dependencies: [],
  navigation: [
    { slot: "tenant.services", id: "mail", label: "Mail", path: "/mail" },
  ],
} as const satisfies ModuleManifest;
/** Includes legacy Studio operations; system authentication email is intentionally separate. */
export function isMailOperation(operation: string): boolean {
  return (
    /^mail[A-Z]/.test(operation) ||
    /^(getTenantMail|manageTenantMail|getTenantSenderDomain|manageTenantSenderDomain|getTenantTracking|manageTenantTracking|launchTenantWebmail|getNativeSending|manageNativeSending)/.test(
      operation,
    ) ||
    [
      "getMailConnectionStatus",
      "saveMailCredentials",
      "clearMailCredentials",
      "testMailConnection",
    ].includes(operation)
  );
}
export function isMailProvisioning(
  operation: string,
  args: unknown[],
): boolean {
  if (
    ["mailConnectByok", "mailConfigureManaged", "mailDomainCreate"].includes(
      operation,
    )
  )
    return true;
  const input = args[1];
  if (!input || typeof input !== "object") return false;
  const action = (input as { action?: unknown }).action;
  return (
    (operation === "manageTenantMail" &&
      ["provision", "link", "add-domain"].includes(String(action))) ||
    (operation === "manageTenantSenderDomain" && action === "register") ||
    (operation === "manageTenantMailboxes" &&
      ["domain.attach", "mailbox.create"].includes(String(action)))
  );
}
