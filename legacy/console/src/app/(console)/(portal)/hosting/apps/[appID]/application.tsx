import { ApplicationServices } from "@/components/hosting/application-services";
import { getHostingApplication } from "@/lib/hosting-application";
import { notFound } from "next/navigation";
import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { formatResource } from "@webdock/hosting-contracts";
import { HostingForm } from "@/components/hosting/form";
import { AppSpecFields } from "@/components/hosting/app-spec";
export default async function Application({
  params,
  view = "overview",
}: {
  params: Promise<{ appID: string }>;
  view?: "overview" | "logs" | "settings";
}) {
  const { appID } = await params;
  const { t, locale } = await getRequestI18n();
  const app = await getHostingApplication(appID);
  if (view === "settings" && !app.canManage && !app.operator) notFound();
  if (app.lifecycle === "draft")
    return (
      <section className="panel">
        <h2>{t("Awaiting first deployment")}</h2>
        <p>{t("This application has not been deployed yet.")}</p>
        <Link
          className="button"
          href={`/hosting/projects/${app.projectID}/git/setup?target=${appID}`}
        >
          {t("Configure deployments")}
        </Link>
        {app.operator && (
          <Link href={`/hosting/apps/${appID}/delete`}>
            {t("Delete application")}
          </Link>
        )}
      </section>
    );
  return (
    <>
      {view === "overview" && (
        <section className="panel">
          <h2>{t("Configured resources")}</h2>
          <dl className="application-resources">
            <div>
              <dt>{t("Replicas")}</dt>
              <dd>{app.spec.replicas}</dd>
            </div>
            <div>
              <dt>{t("CPU cores")}</dt>
              <dd>
                {formatResource(
                  "cpuMillicores",
                  app.spec.cpuMillicores,
                  locale,
                )}
              </dd>
            </div>
            <div>
              <dt>{t("Memory")}</dt>
              <dd>
                {formatResource("memoryBytes", app.spec.memoryBytes, locale)}
              </dd>
            </div>
          </dl>
        </section>
      )}
      {view === "overview" && Boolean(app.spec.volumeBytes) && (
        <section className="panel">
          <h2>{t("Persistent storage")}</h2>
          <p>
            {t(
              "Data is stored at /data and remains after application deletion.",
            )}{" "}
            · {formatResource("volumeBytes", app.spec.volumeBytes ?? 0, locale)}
          </p>
          {app.operator && app.status === "deleted" && (
            <Link
              className="button secondary"
              href={`/hosting/apps/${appID}/storage-delete`}
            >
              {t("Permanently delete retained data")}
            </Link>
          )}
        </section>
      )}
      {view === "overview" && app.status !== "deleted" && (
        <ApplicationServices
          projectID={app.projectID}
          appID={appID}
          canManage={app.canManage}
        />
      )}
      {view === "settings" && app.canManage && (
        <p>
          <Link
            href={`/hosting/projects/${app.projectID}/git/setup?target=${appID}`}
          >
            {t("Configure deployment source")}
          </Link>
        </p>
      )}
      {app.canManage && app.status !== "deleted" && (
        <>
          {view === "overview" && (
            <section className="panel">
              <h2>{t("Application controls")}</h2>
              <div className="application-actions">
                {(["start", "stop", "restart", "rollback"] as const).map(
                  (action, i) => (
                    <HostingForm
                      key={action}
                      command={{
                        action: `apps.${action}`,
                        appID,
                        revision: app.revision,
                        subscriptionRevision: app.subscriptionRevision,
                        idempotencyKey: randomUUID(),
                      }}
                      label={
                        [
                          t("Start application"),
                          t("Stop"),
                          t("Restart"),
                          t("Rollback"),
                        ][i]
                      }
                    />
                  ),
                )}
                <details>
                  <summary>{t("Scale application")}</summary>
                  <HostingForm
                    preserveRevision
                    command={{
                      action: "apps.scale",
                      appID,
                      replicas: app.spec.replicas,
                      revision: app.revision,
                      subscriptionRevision: app.subscriptionRevision,
                      idempotencyKey: randomUUID(),
                    }}
                    label={t("Scale application")}
                  >
                    <label className="field">
                      {t("Replicas")}
                      <input
                        type="number"
                        name="replicas"
                        min="0"
                        max="20"
                        required
                        defaultValue={app.spec.replicas}
                      />
                    </label>
                  </HostingForm>
                </details>
              </div>
            </section>
          )}
          {view === "settings" && (
            <section className="panel">
              <h2>{t("Update application")}</h2>
              <HostingForm
                preserveRevision
                command={{
                  action: "apps.update",
                  appID,
                  spec: app.spec,
                  revision: app.revision,
                  subscriptionRevision: app.subscriptionRevision,
                  idempotencyKey: randomUUID(),
                }}
                label={t("Update application")}
              >
                <AppSpecFields
                  spec={app.spec}
                  storageFixed
                  environmentNames={app.environmentNames}
                  custom={app.operator || app.spec.template === "custom"}
                />
              </HostingForm>
            </section>
          )}
        </>
      )}
      {view === "logs" && app.status !== "deleted" && (
        <section className="panel">
          <h2>{t("Application logs")}</h2>
          {app.canRequestLogs && (
            <HostingForm
              command={{
                action: "apps.logs",
                appID,
                idempotencyKey: randomUUID(),
              }}
              label={t("Refresh logs")}
            />
          )}
          {app.logs === null && <p>{t("No logs have been requested yet.")}</p>}
          {app.logs !== null && (
            <pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
              {app.logs || t("No log output.")}
            </pre>
          )}
        </section>
      )}
      {view === "settings" && app.operator && app.status !== "deleted" && (
        <section className="panel">
          <h2>{t("Operator recovery")}</h2>
          <HostingForm
            command={{
              action: "apps.reconcile",
              appID,
              idempotencyKey: randomUUID(),
            }}
            label={t("Reconcile operation")}
          />
          <Link
            className="button secondary"
            href={`/hosting/apps/${appID}/delete`}
          >
            {t("Delete application")}
          </Link>
        </section>
      )}
    </>
  );
}
