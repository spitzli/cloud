import type { DatabaseRuntimePorts } from "./ports";
import { X509Certificate } from "node:crypto";
import { HostingError } from "@spitzli/hosting-contracts";
import type { Connection } from "@spitzli/hosting-contracts/runtime-ports";
export type Dependencies = {
  requireDatabases: DatabaseRuntimePorts["requireDatabases"];
  decryptSecret: DatabaseRuntimePorts["decryptSecret"];
};

export function createHostingDatabaseBinding(dependencies: Dependencies) {
  function databaseConnectionURL(input: {
    host: string;
    port: number;
    database: string;
    username: string;
    password: string;
    caPath: string;
  }) {
    if (
      !/^[a-z0-9][a-z0-9.-]+$/.test(input.host) ||
      !Number.isInteger(input.port) ||
      input.port < 1 ||
      input.port > 65535 ||
      !/^\/var\/run\/webdock\/services\/[1-9][0-9]*\/ca\.crt$/.test(
        input.caPath,
      )
    )
      throw new HostingError(
        409,
        "Verified database connection details are not available yet.",
      );
    const url = new URL(`postgresql://${input.host}:${input.port}`);
    url.username = encodeURIComponent(input.username);
    url.password = encodeURIComponent(input.password);
    url.pathname = "/" + encodeURIComponent(input.database);
    url.searchParams.set("sslmode", "verify-full");
    url.searchParams.set("sslrootcert", input.caPath);
    return url.href;
  }
  async function databaseBindingMaterial(
    db: Connection,
    input: {
      databaseID: string;
      projectID: string;
      customerID: string;
      clusterID: string;
      bindingID: string;
      variable: string;
    },
  ) {
    dependencies.requireDatabases("access");
    const row = (
      await db.query(
        `SELECT d.*,b.cluster_id,b.ca_certificate,b.endpoint_ready,b.state AS backend_state
    FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id
    WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3 FOR SHARE OF d,b`,
        [input.databaseID, input.projectID, input.customerID],
      )
    ).rows[0];
    if (!row || row.cluster_id !== input.clusterID)
      throw new HostingError(
        404,
        "A database in the same project and cluster is required.",
      );
    if (
      row.state !== "ready" ||
      !row.endpoint_ready ||
      row.backend_state !== "ready" ||
      !row.ca_certificate ||
      !row.confirmed_credentials_encrypted ||
      row.confirmed_credential_revision !== row.credential_revision
    )
      throw new HostingError(
        409,
        "Wait for confirmed database readiness before connecting the application.",
      );
    try {
      new X509Certificate(row.ca_certificate);
    } catch {
      throw new HostingError(
        409,
        "Verified database connection details are not available yet.",
      );
    }
    const secret = dependencies.decryptSecret<{
      applicationPassword: string;
    }>(
      row.confirmed_credentials_encrypted,
      `database:${row.id}:${row.confirmed_credential_revision}`,
    );
    return {
      credentialRevision: row.confirmed_credential_revision as number,
      environment: {
        [input.variable]: databaseConnectionURL({
          host: `postgres-rw.wd-db-${row.backend_id}.svc.cluster.local`,
          port: 5432,
          database: row.database_name,
          username: "wd_app_" + row.id,
          password: secret.applicationPassword,
          caPath: `/var/run/webdock/services/${input.bindingID}/ca.crt`,
        }),
      },
      runtime: {
        id: input.bindingID,
        revision: 1,
        backendID: row.backend_id as string,
        caCertificate: row.ca_certificate as string,
      },
    };
  }
  return { databaseConnectionURL, databaseBindingMaterial };
}
