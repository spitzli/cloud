import type { DatabaseRuntimePorts } from "./ports";
import { HostingError, type HostingActor } from "@spitzli/hosting-contracts";
export type Dependencies = {
  withHostingDatabase: DatabaseRuntimePorts["withHostingDatabase"];
  transaction: DatabaseRuntimePorts["transaction"];
  audit: DatabaseRuntimePorts["audit"];
  authorizeHosting: DatabaseRuntimePorts["authorizeHosting"];
  databaseAccess: ReturnType<
    typeof import("./databases").createHostingDatabases
  >["databaseAccess"];
  decryptSecret: DatabaseRuntimePorts["decryptSecret"];
};

export function createHostingDatabaseCredentials(dependencies: Dependencies) {
  async function displayDatabaseCredentials(
    actor: HostingActor,
    databaseID: string,
  ) {
    return dependencies.withHostingDatabase(actor, async () => {
      if (actor.source !== "studio")
        throw new HostingError(403, "Open database credentials in Studio.");
      return dependencies.transaction(async (db) => {
        const d = await dependencies.databaseAccess(
          db,
          actor,
          databaseID,
          true,
        );
        await dependencies.authorizeHosting(
          actor,
          {
            permission: "org:databases:manage",
            projectID: d.project_id,
            write: true,
            tenantAdmin: true,
            live: true,
          },
          db,
        );
        if (
          !["ready", "paused"].includes(d.state) ||
          !d.confirmed_credentials_encrypted ||
          d.confirmed_credential_revision !== d.credential_revision
        )
          throw new HostingError(
            409,
            "Confirmed database credentials are not available yet.",
          );
        const backend = (
          await db.query(
            "SELECT host,direct_port,pooled_port,endpoint_ready,ca_certificate FROM webdock_auth.hosting_database_backend WHERE id=$1",
            [d.backend_id],
          )
        ).rows[0];
        if (
          !backend ||
          (!backend.endpoint_ready && d.state !== "paused") ||
          !backend.ca_certificate
        )
          throw new HostingError(
            409,
            "Verified database connection details are not available yet.",
          );
        const secret = dependencies.decryptSecret<{
          applicationPassword: string;
          migrationPassword: string;
        }>(
          d.confirmed_credentials_encrypted,
          `database:${d.id}:${d.confirmed_credential_revision}`,
        );
        await dependencies.audit(
          db,
          actor,
          "databases.credentials.display",
          d.id,
        );
        return {
          databaseID: d.id,
          databaseName: d.database_name,
          host: backend.host,
          directPort: backend.direct_port,
          pooledPort: backend.pooled_port,
          credentialRevision: d.confirmed_credential_revision,
          application: {
            username: "wd_app_" + d.id,
            password: secret.applicationPassword,
          },
          migration: {
            username: "wd_migrate_" + d.id,
            password: secret.migrationPassword,
          },
          sslmode: "verify-full",
          caCertificate: backend.ca_certificate,
        };
      });
    });
  }
  return { displayDatabaseCredentials };
}
