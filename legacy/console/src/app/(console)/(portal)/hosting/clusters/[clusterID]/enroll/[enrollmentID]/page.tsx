import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { EnrollmentForm } from "@/components/hosting/form";
export default async function Enrollment({
  params,
}: {
  params: Promise<{ clusterID: string; enrollmentID: string }>;
}) {
  const { clusterID, enrollmentID } = await params,
    { t } = await getRequestI18n();
  await hostingPageCall(
    { action: "byok.cluster", clusterID },
    `/hosting/clusters/${clusterID}`,
  );
  const endpoint = new URL(
    process.env.WEBDOCK_AUTH_ISSUER || "https://auth.webdock.dev",
  ).origin;
  return (
    <section className="panel">
      <h1>{t("Connect your cluster")}</h1>
      <ol className="hosting-setup">
        <li>
          <h2>{t("Download the agent")}</h2>
          <p>
            {t(
              "Use a Linux host with Python 3, kubectl and access to your Kubernetes cluster.",
            )}
          </p>
          <a className="button secondary" href="/hosting-agent.zip" download>
            {t("Download agent")}
          </a>
        </li>
        <li>
          <h2>{t("Prepare cluster access")}</h2>
          <p>
            {t(
              "Use a dedicated kubeconfig with the permissions in the included RBAC guide. Keep it on your host; never upload it to Webdock.",
            )}
          </p>
        </li>
        <li>
          <h2>{t("Install and connect")}</h2>
          <p>
            {t(
              "Extract the archive, run the installer and enter the one-time token when prompted.",
            )}
          </p>
          <pre className="hosting-logs">{`sudo python3 install.py --endpoint ${endpoint} --cluster ${clusterID} --kubeconfig /path/to/kubeconfig`}</pre>
          <EnrollmentForm enrollmentID={enrollmentID} />
        </li>
      </ol>
      <Link
        className="button secondary"
        href={`/hosting/clusters/${clusterID}`}
      >
        {t("Check connection")}
      </Link>
    </section>
  );
}
