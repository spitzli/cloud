import type { Connection } from "./authorization";
import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import {
  HostingError,
  serviceCommandSchema,
  type HostingActor,
  type ServiceBindingView,
} from "@spitzli/hosting-contracts";
import type { BindingMaterial } from "./binding-runtime";
import { limits } from "./allowances";
export type Dependencies = {
  applicationCapabilities: ReturnType<
    typeof import("./setup-capabilities").createHostingSetupCapabilities
  >["applicationCapabilities"];
  mailBindingMaterial: ReturnType<
    typeof import("./mail-binding").createHostingMailBinding
  >["mailBindingMaterial"];
  revokeBindingMail: ReturnType<
    typeof import("./mail-binding").createHostingMailBinding
  >["revokeBindingMail"];
  bindingMailReady: ReturnType<
    typeof import("./mail-binding").createHostingMailBinding
  >["bindingMailReady"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  recordResult: ReturnType<
    typeof import("./operations").createHostingOperations
  >["recordResult"];
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
  sealEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["sealEnvironment"];
  encryptMailSecret: HostingPorts["encryptMailSecret"];
  databaseBindingMaterial: DatabaseServices["databaseBindingMaterial"];
  openBinding: ReturnType<
    typeof import("./binding-runtime").createHostingBindingRuntime
  >["openBinding"];
  executeAppCommand: ReturnType<
    typeof import("./apps").createHostingApps
  >["executeAppCommand"];
};

export function createHostingServiceBindings(dependencies: Dependencies) {
  function view(row: {
    id: ServiceBindingView["id"];
    project_id: ServiceBindingView["projectID"];
    app_id: ServiceBindingView["appID"];
    service: ServiceBindingView["service"];
    revision: ServiceBindingView["revision"];
    observed_revision: ServiceBindingView["observedRevision"];
    credential_revision: ServiceBindingView["credentialRevision"];
    state: ServiceBindingView["state"];
    reason: ServiceBindingView["reason"];
  }): ServiceBindingView {
    return {
      id: row.id,
      projectID: row.project_id,
      appID: row.app_id,
      service: row.service,
      revision: row.revision,
      observedRevision: row.observed_revision,
      credentialRevision: row.credential_revision,
      state: row.state,
      reason: row.reason,
    };
  }
  async function executeServiceBinding(actor: HostingActor, raw: unknown) {
    const cmd = serviceCommandSchema.parse(raw);
    return dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: cmd.projectID,
          write: cmd.action !== "services.list",
        },
        db,
      );
      if (cmd.action === "services.list") {
        return readServiceBindings(
          db,
          cmd.projectID,
          access.customerID!,
          cmd.appID,
        );
      }
      await dependencies.planLock(db, access.customerID!);
      const request = await dependencies.replay(
        db,
        actor,
        cmd.idempotencyKey,
        cmd,
      );
      if (request.existing)
        return request.existing.result as ServiceBindingView;
      let row =
        cmd.action === "services.bind"
          ? (
              await db.query(
                "SELECT * FROM webdock_auth.hosting_service_binding WHERE project_id=$1 AND app_id=$2 AND variable=$3 AND state<>'disconnected' FOR UPDATE",
                [cmd.projectID, cmd.appID, cmd.service.variable],
              )
            ).rows[0]
          : (
              await db.query(
                "SELECT * FROM webdock_auth.hosting_service_binding WHERE id=$1 AND project_id=$2 FOR UPDATE",
                [cmd.bindingID, cmd.projectID],
              )
            ).rows[0];
      if (cmd.action !== "services.bind" && !row)
        throw new HostingError(404, "Service connection is unavailable.");
      if ((row?.revision ?? 0) !== cmd.revision)
        throw new HostingError(
          409,
          "Service connection changed. Refresh before editing.",
        );
      const app = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_app WHERE id=$1 AND project_id=$2 AND status<>'deleted' FOR UPDATE",
          [
            row?.app_id ?? (cmd.action === "services.bind" ? cmd.appID : null),
            cmd.projectID,
          ],
        )
      ).rows[0];
      if (!app || !access.project?.cluster_id)
        throw new HostingError(404, "Application is unavailable.");
      const environment = dependencies.openEnvironment(
        app.id,
        app.environment_encrypted,
      );
      const service =
        cmd.action === "services.bind" ? cmd.service : row.service;
      if (service.kind === "postgres")
        await dependencies.authorizeHosting(
          actor,
          {
            projectID: cmd.projectID,
            write: true,
            permission: "org:databases:manage",
          },
          db,
        );
      if (cmd.action === "services.rotate" && service.kind !== "mail")
        throw new HostingError(
          409,
          "Rotate database credentials from the database settings.",
        );
      if (row?.retiring_mail_key_id && cmd.action !== "services.disconnect")
        throw new HostingError(
          409,
          "Wait for the current Mail key rollout before changing this connection.",
        );
      const oldNames = row?.secret_encrypted
        ? Object.keys(
            dependencies.openBinding(row.id, row.secret_encrypted).environment,
          )
        : [];
      if (cmd.action === "services.disconnect") {
        if (service.kind === "mail")
          await dependencies.revokeBindingMail(db, row.id);
        for (const name of oldNames) delete environment[name];
        row = (
          await db.query(
            "UPDATE webdock_auth.hosting_service_binding SET state=$2,revision=revision+1,secret_encrypted=NULL,reason=NULL WHERE id=$1 RETURNING *",
            [
              row.id,
              app.lifecycle === "draft" ? "disconnected" : "disconnecting",
            ],
          )
        ).rows[0];
      } else {
        const capable = {
          version: String(
            (
              await dependencies.applicationCapabilities(
                db,
                access.project.cluster_id,
              )
            )?.serviceBindingsVersion,
          ),
        };
        if (capable?.version !== "1")
          throw new HostingError(
            409,
            "Update the cluster agent before connecting services.",
          );
        if (!row && Object.hasOwn(environment, service.variable))
          throw new HostingError(
            409,
            "An environment variable already uses this name.",
          );
        if (
          !row &&
          (
            await db.query(
              "SELECT count(*)::int AS n FROM webdock_auth.hosting_service_binding WHERE app_id=$1 AND state<>'disconnected'",
              [app.id],
            )
          ).rows[0].n >= 16
        )
          throw new HostingError(
            409,
            "This application already has sixteen service connections.",
          );
        const id =
          row?.id ??
          (await db.query("SELECT webdock_auth.next_snowflake() AS id")).rows[0]
            .id;
        const material: BindingMaterial =
          service.kind === "mail"
            ? await dependencies.mailBindingMaterial(
                db,
                actor,
                {
                  customerID: access.customerID!,
                  connectionID: service.connectionID,
                  domainID: service.domainID,
                  bindingID: id,
                  variable: service.variable,
                },
                cmd.action !== "services.rotate" && row?.secret_encrypted
                  ? dependencies.openBinding(row.id, row.secret_encrypted)
                  : undefined,
              )
            : await dependencies.databaseBindingMaterial(db, {
                databaseID: service.databaseID,
                projectID: cmd.projectID,
                customerID: access.customerID!,
                clusterID: access.project.cluster_id,
                bindingID: id,
                variable: service.variable,
              });
        for (const name of Object.keys(material.environment))
          if (Object.hasOwn(environment, name) && !oldNames.includes(name))
            throw new HostingError(
              409,
              "An environment variable already uses this name.",
            );
        if (
          row?.mail_key_id &&
          row.mail_key_id !== material.mailKeyID &&
          app.lifecycle === "draft"
        )
          await dependencies.revokeBindingMail(db, row.id);
        const retiringKeyID =
          row?.mail_key_id &&
          row.mail_key_id !== material.mailKeyID &&
          app.lifecycle !== "draft"
            ? row.mail_key_id
            : null;
        if (service.kind === "mail")
          material.credentialRevision =
            row?.mail_key_id === material.mailKeyID
              ? row.credential_revision
              : (row?.credential_revision ?? 0) + 1;
        for (const name of oldNames) delete environment[name];
        Object.assign(environment, material.environment);
        const sealed = dependencies.encryptMailSecret(
          material,
          "hosting-binding:" + id,
        );
        row = (
          await db.query(
            `INSERT INTO webdock_auth.hosting_service_binding(id,customer_id,project_id,app_id,service,variable,credential_revision,secret_encrypted,mail_key_id,retiring_mail_key_id)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(id) DO UPDATE SET service=$5,credential_revision=$7,secret_encrypted=$8,mail_key_id=$9,retiring_mail_key_id=$10,revision=hosting_service_binding.revision+1,state='pending',reason=NULL RETURNING *`,
            [
              id,
              access.customerID,
              cmd.projectID,
              app.id,
              JSON.stringify(service),
              service.variable,
              material.credentialRevision,
              sealed,
              material.mailKeyID ?? null,
              retiringKeyID,
            ],
          )
        ).rows[0];
      }
      const encrypted = dependencies.sealEnvironment(app.id, environment);
      if (app.lifecycle === "draft")
        await db.query(
          "UPDATE webdock_auth.hosting_app SET environment_encrypted=$2,revision=revision+1 WHERE id=$1",
          [app.id, encrypted],
        );
      else {
        const patch = environment;
        await dependencies.executeAppCommand(
          actor,
          {
            action: "apps.update",
            appID: app.id,
            revision: app.revision,
            spec: app.spec,
            environment: [
              ...Object.entries(patch).map(([name, value]) => ({
                name,
                value,
              })),
              ...oldNames
                .filter((name) => !(name in patch))
                .map((name) => ({ name, value: null })),
            ],
            subscriptionRevision: (await limits(db, access.customerID!))
              .subscriptionRevision,
            idempotencyKey: "binding-" + row.id + "-" + row.revision,
          },
          db,
        );
      }
      const result = view(row);
      await dependencies.recordResult(
        db,
        actor,
        cmd.idempotencyKey,
        request.hash,
        cmd.action,
        result,
        { customerID: access.customerID, projectID: cmd.projectID },
      );
      await dependencies.audit(db, actor, cmd.action, row.id);
      return result;
    });
  }
  /** Caller holds the project's live authorization locks in this transaction. */
  async function readServiceBindings(
    db: Connection,
    projectID: string,
    customerID: string,
    appID?: string,
  ) {
    const rows = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_service_binding WHERE project_id=$1 AND customer_id=$2 AND ($3::text IS NULL OR app_id=$3) ORDER BY id",
        [projectID, customerID, appID ?? null],
      )
    ).rows;
    for (const row of rows)
      if (
        row.service.kind === "postgres" &&
        row.secret_encrypted &&
        !["disconnected", "disconnecting"].includes(row.state)
      ) {
        const material = dependencies.openBinding(row.id, row.secret_encrypted);
        const current = (
          await db.query(
            "SELECT d.state,d.confirmed_credential_revision,d.credential_revision,b.ca_certificate,b.endpoint_ready FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3",
            [row.service.databaseID, projectID, customerID],
          )
        ).rows[0];
        if (
          !current ||
          current.state !== "ready" ||
          !current.endpoint_ready ||
          current.credential_revision !== row.credential_revision ||
          current.confirmed_credential_revision !== row.credential_revision ||
          current.ca_certificate !==
            ("caCertificate" in material.runtime
              ? material.runtime.caCertificate
              : null)
        ) {
          row.state = "blocked";
          row.reason = "Refresh the database connection before deployment.";
        }
      }
    for (const row of rows)
      if (
        row.service.kind === "mail" &&
        !["disconnected", "disconnecting"].includes(row.state) &&
        !(await dependencies.bindingMailReady(db, row.id))
      ) {
        row.state = "blocked";
        row.reason =
          "Native Mail connection needs attention before deployment.";
      }
    return rows.map(view);
  }

  return { executeServiceBinding, readServiceBindings };
}
