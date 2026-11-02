import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingRefresh } from "@/components/hosting/refresh";
export default async function DeleteStorage({
  params,
}: {
  params: Promise<{ appID: string }>;
}) {
  const { appID } = await params;
  const { t } = await getRequestI18n();
  const preview = await hostingPageCall<{
    plan: { name: string; namespace: string; resources: string[] };
    planHash: string;
  } | {removed:true;name:string}>(
    { action: "apps.storageDeletion", appID },
    `/hosting/apps/${encodeURIComponent(appID)}/storage-delete`,
  );
  if ('removed' in preview) return <section className="panel"><h1>{preview.name}</h1><p>{t("Retained data has been permanently removed. The storage allowance is available again.")}</p><Link href={`/hosting/apps/${appID}`}>{t("Open application")}</Link></section>;
  return (
    <section className="panel">
      <HostingRefresh active />
      <h1>
        {t("Permanently delete retained data")}: {preview.plan.name}
      </h1>
      <p>
        {t(
          "This permanently removes the retained filesystem and all its data. This cannot be undone. The application must already be deleted.",
        )}
      </p>
      <p>
        {preview.plan.namespace} · {preview.plan.resources.join(", ")}
      </p>
      <HostingForm
        command={{
          action: "apps.purgeStorage",
          appID,
          confirmName: "",
          planHash: preview.planHash,
          idempotencyKey: randomUUID(),
        }}
        label={t("Permanently delete retained data")}
      >
        <label className="field">
          {t("Type the exact application name")}
          <input name="confirmName" required autoComplete="off" />
        </label>
      </HostingForm>
    </section>
  );
}
