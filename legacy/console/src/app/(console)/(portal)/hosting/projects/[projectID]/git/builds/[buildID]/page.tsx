import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingRefresh } from "@/components/hosting/refresh";
export default async function Build({
  params,
}: {
  params: Promise<{ projectID: string; buildID: string }>;
}) {
  const { projectID, buildID } = await params,
    { t } = await getRequestI18n();
  const path = `/hosting/projects/${projectID}/git`;
  const build = await hostingPageCall<{
    sourceSHA: string;
    status: string;
    logs: string;
    actionsRunURL: string | null;
    failureCode: string | null;
  }>(
    { action: "git.builds.get", projectID, buildID },
    `${path}/builds/${buildID}`,
  );
  return (
    <>
      <Link href={path}>{t("Git deployments")}</Link>
      <h1>{t("Build logs")}</h1>
      <HostingRefresh active={["queued", "running"].includes(build.status)} />
      <section className="panel">
        <p>
          <code>{build.sourceSHA}</code>
        </p>
        <p role="status">{t(build.status)}</p>
        {build.failureCode && (
          <p role="alert">
            {t("Build failed. Review the logs and setup requirements.")}{" "}
            <code>{build.failureCode}</code>
          </p>
        )}
        {build.actionsRunURL && (
          <p>
            <a
              href={build.actionsRunURL}
              target="_blank"
              rel="noopener noreferrer"
            >
              {t("View build logs in GitHub Actions")}
            </a>
          </p>
        )}
        <pre
          style={{
            whiteSpace: "pre-wrap",
            overflowWrap: "anywhere",
            maxHeight: "70vh",
            overflow: "auto",
          }}
        >
          {build.logs || t("No build output yet.")}
        </pre>
      </section>
    </>
  );
}
