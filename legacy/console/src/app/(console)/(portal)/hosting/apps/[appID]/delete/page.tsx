import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
export default async function DeleteApplication({
  params,
}: {
  params: Promise<{ appID: string }>;
}) {
  const { appID } = await params;
  const { t } = await getRequestI18n();
  const preview = await hostingPageCall<{
    plan: { name: string; namespace: string; resources: string[] };
    planHash: string;
  }>(
    { action: "apps.deletion", appID },
    `/hosting/apps/${encodeURIComponent(appID)}/delete`,
  );
  return (
    <section className="panel">
      <h1>
        {t("Delete application")}: {preview.plan.name}
      </h1>
      <p>
        {t(
          "This removes the application deployment, its pods and its service. The managed project namespace and persistent data remain. Retained storage still counts against the allowance. Only a platform operator can confirm deletion.",
        )}
      </p>
      <p>
        {preview.plan.namespace} · {preview.plan.resources.join(", ")}
      </p>
      <HostingForm
        command={{
          action: "apps.delete",
          appID,
          confirmName: "",
          planHash: preview.planHash,
          idempotencyKey: randomUUID(),
        }}
        label={t("Permanently delete application")}
      >
        <label className="field">
          {t("Type the exact application name")}
          <input name="confirmName" required autoComplete="off" />
        </label>
      </HostingForm>
    </section>
  );
}
