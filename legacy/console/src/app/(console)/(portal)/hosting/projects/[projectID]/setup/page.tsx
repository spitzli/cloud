import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import type { SetupOptions, SetupView } from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { SetupWizard } from "@/components/hosting/setup-wizard";
import { SetupHosting } from "@/components/hosting/setup-hosting";
import { SetupMailPanel } from "@/components/hosting/setup-mail-panel";
export default async function Page({
  params,
}: {
  params: Promise<{ projectID: string }>;
}) {
  const { projectID } = await params,
    path = `/hosting/projects/${projectID}/setup`,
    { t } = await getRequestI18n();
  const [options, drafts] = await Promise.all([
    hostingPageCall<SetupOptions>({ action: "setup.options", projectID }, path),
    hostingPageCall<{ docs: SetupView[] }>(
      { action: "setup.list", projectID, page: 1, limit: 5 },
      path,
    ),
  ]);
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("Add application")}</h1>
          <p>
            {t(
              "Connect a source, choose services and review everything before deployment.",
            )}
          </p>
        </div>
      </div>
      {drafts.docs.some((d) => d.state !== "ready") && (
        <details className="workspace-disclosure">
          <summary>{t("Continue an existing setup")}</summary>
          <ul className="workspace-list">
            {drafts.docs
              .filter((d) => d.state !== "ready")
              .map((d) => (
                <li key={d.id}>
                  <Link href={path + "/" + d.id}>{d.configuration.name}</Link>
                </li>
              ))}
          </ul>
        </details>
      )}
      {!options.project.configured || options.project.provider !== "k3s" ? (
        <SetupHosting options={options} />
      ) : !options.project.canManage ? (
        <p>{t("Application setup requires project management access.")}</p>
      ) : (
        <SetupWizard
          options={options}
          requestKey={randomUUID()}
          mailSetup={<SetupMailPanel options={options} />}
        />
      )}
    </>
  );
}
