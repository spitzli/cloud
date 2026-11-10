import type { SetupConfiguration } from "@spitzli/hosting-contracts";
import type { HostingPorts } from "./ports";
/** Only new admission needs provisioning authority; a confirmed setup can finish while products drain. */
export function assertSetupProducts(
  ports: Pick<HostingPorts, "requireDatabases" | "requireMail">,
  configuration: Pick<SetupConfiguration, "database" | "mail">,
  accepted: { confirmed?: boolean; databaseID?: string } = {},
): void {
  if (configuration.database.mode !== "none")
    ports.requireDatabases(
      configuration.database.mode === "new" &&
        !accepted.confirmed &&
        !accepted.databaseID
        ? "provision"
        : "access",
    );
  if (configuration.mail) ports.requireMail("access");
}
