import { setupStage } from "@/lib/project-setup-view";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import {
  hostingError,
  type SetupOptions,
  type SetupView,
  type SetupInspection,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { SetupWizard } from "@/components/hosting/setup-wizard";
import { SetupProgress } from "@/components/hosting/setup-progress";
import { SetupMailPanel } from "@/components/hosting/setup-mail-panel";
export default async function Page({
  params,
  searchParams,
}: {
  params: Promise<{ projectID: string; setupID: string }>;
  searchParams: Promise<{
    step?: string;
    edit?: string;
    git?: string;
    inspect?: string;
  }>;
}) {
  const { projectID, setupID } = await params,
    query = await searchParams,
    path = `/hosting/projects/${projectID}/setup/${setupID}`,
    { t } = await getRequestI18n();
  const [view, options] = await Promise.all([
    hostingPageCall<SetupView>(
      { action: "setup.get", projectID, setupID },
      path,
    ),
    hostingPageCall<SetupOptions>({ action: "setup.options", projectID }, path),
  ]);
  const editing =
    setupStage(view.state) !== "progress" ||
    (query.edit === "1" &&
      !Object.keys(view.resources).length &&
      view.state !== "running");
  if (!editing || !options.project.canManage)
    return (
      <SetupProgress
        view={view}
        canManage={options.project.canManage}
        canRepairEnvironment={
          view.state === "blocked" &&
          view.steps.some(
            (s) => s.name === "bindings" && s.state === "blocked",
          ) &&
          options.applications.some(
            (a) => a.id === view.resources.appID && a.lifecycle === "draft",
          )
        }
      />
    );
  let inspection: SetupInspection | undefined,
    inspectionError: string | undefined;
  if (query.inspect === "1")
    try {
      inspection = await hostingPageCall<SetupInspection>(
        {
          action: "setup.inspectSource",
          projectID,
          setupID,
          revision: view.revision,
        },
        path,
      );
    } catch (error) {
      inspectionError = hostingError(error).message;
    }
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("Set up application")}</h1>
          <p>{view.configuration.name}</p>
        </div>
      </div>
      {query.git === "connected" && (
        <p className="notice" role="status">
          {t("GitHub is connected. Review your repository and continue.")}
        </p>
      )}
      {query.git === "failed" && (
        <p className="notice error" role="alert">
          {t("GitHub connection was not completed. Your draft has been kept.")}
        </p>
      )}
      <SetupWizard
        initialInspection={inspection}
        initialError={inspectionError}
        options={options}
        initial={view}
        initialStep={/^[0-3]$/.test(query.step ?? "") ? Number(query.step) : 0}
        requestKey={randomUUID()}
        mailSetup={<SetupMailPanel options={options} />}
      />
    </>
  );
}
