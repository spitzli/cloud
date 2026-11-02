import { getRequestI18n } from "@webdock/i18n/next";
import { EnrollmentForm } from "@/components/hosting/form";
export default async function Enrollment({
  params,
}: {
  params: Promise<{ enrollmentID: string }>;
}) {
  const { t } = await getRequestI18n();
  return (
    <section className="panel">
      <h1>{t("Connect cluster agent")}</h1>
      <p>
        {t(
          "Cluster management is being configured. Application deployment is not available yet.",
        )}
      </p>
      <EnrollmentForm enrollmentID={(await params).enrollmentID} />
    </section>
  );
}
