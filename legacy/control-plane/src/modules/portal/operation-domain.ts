import { moduleForOperation } from "@webdock/module-sdk/selection";
/** Compatibility operation names do not determine the owning product domain. */
export function studioOperationDomain(operation: string, args: unknown[]): string {
  if (operation === "hosting.command" || operation === "database.command")
    return moduleForOperation(operation, args)!;
  for (const [prefix, domain] of [
    ["project.", "lifecycle"],
    ["registry.", "registry"], ["integrations.", "integrations"], ["cms.", "cms"],
  ]) if (operation.startsWith(prefix)) return domain;
  return moduleForOperation(operation, args) === "mail" ? "mail" : "auth";
}
