import * as React from "react";
import { SetupEnvironmentRepair } from "./setup-environment-repair";
import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@spitzli/i18n/next";
import { msgid } from "@spitzli/i18n";
import type { SetupView } from "@spitzli/hosting-contracts";
import { HostingRefresh } from "./refresh";
import { HostingForm } from "./form";
const labels = {
  application: msgid("Prepare application"),
  database: msgid("Prepare database"),
  bindings: msgid("Connect services"),
  source: msgid("Configure source"),
  build: msgid("Build application"),
  release: msgid("Deploy release"),
  readiness: msgid("Verify readiness"),
};
const states = {
  pending: msgid("Pending"),
  running: msgid("In progress"),
  blocked: msgid("Action required"),
  failed: msgid("Failed"),
  succeeded: msgid("Complete"),
  skipped: msgid("Not selected"),
};
export async function SetupProgress({
  view,
  canManage,
  canRepairEnvironment = false,
}: {
  view: SetupView;
  canManage: boolean;
  canRepairEnvironment?: boolean;
}) {
  const { t, error } = await getRequestI18n();
  const base = `/hosting/projects/${view.projectID}`;
  return (
    <>
      <HostingRefresh active={view.state === "running"} interval={5000} />
      <div className="page-heading">
        <div>
          <h1>{view.configuration.name}</h1>
          <p>
            {view.state === "ready"
              ? t("Your application and service connections are ready.")
              : t(
                  "Follow the deployment of your application and its services.",
                )}
          </p>
        </div>
        <Link className="button secondary" href={base + "/overview"}>
          {t("Project overview")}
        </Link>
      </div>
      {view.reason && (
        <p className="notice error" role="alert">
          {error(view.reason)}
        </p>
      )}
      <section className="panel">
        <ol className="setup-progress" aria-live="polite">
          {view.steps.map((step) => (
            <li key={step.name} data-state={step.state}>
              <span className="setup-progress-marker" aria-hidden="true">
                {step.state === "succeeded"
                  ? "✓"
                  : step.state === "blocked" || step.state === "failed"
                    ? "!"
                    : "•"}
              </span>
              <div>
                <strong>{t(labels[step.name])}</strong>
                {step.reason && <p>{error(step.reason)}</p>}
              </div>
              <span className="badge">{t(states[step.state])}</span>
            </li>
          ))}
        </ol>
      </section>
      <div className="workspace-actions">
        {view.resources.appID && (
          <Link
            className="button"
            href={`/hosting/apps/${view.resources.appID}`}
          >
            {t("Open application")}
          </Link>
        )}
        {view.resources.buildID && (
          <Link
            className="button secondary"
            href={`${base}/git/builds/${view.resources.buildID}`}
          >
            {t("Build logs")}
          </Link>
        )}
        {view.resources.releaseID && (
          <Link
            className="button secondary"
            href={`${base}/git?target=${view.resources.appID}`}
          >
            {t("Review release")}
          </Link>
        )}
        {view.resources.operationID && (
          <Link
            className="button secondary"
            href={`/hosting/operations/${view.resources.operationID}`}
          >
            {t("View progress")}
          </Link>
        )}
        {view.resources.databaseID && (
          <Link
            className="button secondary"
            href={`/hosting/databases/${view.resources.databaseID}`}
          >
            {t("Open database")}
          </Link>
        )}
        {!Object.keys(view.resources).length &&
          view.state !== "running" &&
          canManage && (
            <Link
              className="button secondary"
              href={`${base}/setup/${view.id}?edit=1`}
            >
              {t("Edit setup")}
            </Link>
          )}
      </div>
      {canManage && canRepairEnvironment && (
        <SetupEnvironmentRepair view={view} requestKey={randomUUID()} />
      )}
      {canManage && ["blocked", "failed"].includes(view.state) && (
        <section className="panel">
          <h2>{t("Continue setup")}</h2>
          <p>
            {t(
              "Resolve the indicated issue, then continue. Already created services and their data are reused.",
            )}
          </p>
          {view.configuration.database.mode === "new" &&
            !view.resources.databaseID && (
              <HostingForm
                command={{
                  action: "setup.preview",
                  projectID: view.projectID,
                  setupID: view.id,
                  revision: view.revision,
                }}
                label={t("Review setup")}
              />
            )}
          <HostingForm
            command={{
              action: "setup.resume",
              projectID: view.projectID,
              setupID: view.id,
              revision: view.revision,
              idempotencyKey: randomUUID(),
            }}
            label={t("Retry setup")}
          />
        </section>
      )}
    </>
  );
}
