import { installationRegistry, publicInstallation } from "../../installation";
export function requireDatabases(intent: "access" | "provision" = "access") {
  installationRegistry().require("databases", intent);
}

export function setupProductAvailable(
  product: "mail" | "databases",
  env: Record<string, string | undefined> = process.env,
): boolean {
  return ["enabled", "draining"].includes(
    publicInstallation(env).modules[product] ?? "disabled",
  );
}
