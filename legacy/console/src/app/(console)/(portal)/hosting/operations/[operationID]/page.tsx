import Link from "next/link";
import { uiLabel } from "@/lib/ui-labels";
import { HostingForm } from "@/components/hosting/form";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingRefresh } from "@/components/hosting/refresh";
import { HostingProjectHeader } from '@/components/hosting/project-context';
import { WorkspaceContext } from '@/components/workspace-context';
import { getTenant } from '@/lib/tenants';
import { headers } from 'next/headers';
export default async function Operation({
  params,
}: {
  params: Promise<{ operationID: string }>;
}) {
  const { operationID } = await params,
    { t } = await getRequestI18n();
  const op = await hostingPageCall<{ status: string; customerID?: string; projectID?: string; resourceName?: string; result?: { error?: string; logs?: string }; reconciliation?: { resourceVersion: string }; clusterID?: string }>(
    { action: "operations.get", operationID },
    `/hosting/operations/${operationID}`,
  );
  const customer = !op.projectID && op.customerID ? await getTenant(await headers(),op.customerID) : null;
  return (
    <>
    {op.projectID ? <HostingProjectHeader projectID={op.projectID} leaf={{label:t('Operation progress')}} showNavigation={false}/> : <WorkspaceContext items={customer ? [{label:customer.tenant.name,href:`/tenants/${op.customerID}`},{label:t('Operation progress')}] : [{label:t('Operations'),href:'/operations'},{label:t('Operation progress')}]} />}
    <section className="panel">
      <h1>{t("Operation progress")}</h1>
      {op.projectID && <p><Link href={`/hosting/projects/${op.projectID}`}>{t("Applications")}</Link></p>}
      {op.resourceName && <h2>{op.resourceName}</h2>}
      <p role="status">{t(uiLabel(op.status))}</p>
      <HostingRefresh active={["queued", "running"].includes(op.status)} />
      {op.result?.error && <p role="alert">{t(op.result.error)}</p>}
      {typeof op.result?.logs === "string" && (
        <pre className="hosting-logs">
          {op.result.logs || t("No logs available.")}
        </pre>
      )}
      {op.reconciliation && (
        <HostingForm
          command={{
            action: "byok.reconcile",
            operationID,
            resourceVersion: op.reconciliation.resourceVersion,
          }}
          label={t("Confirm reviewed state")}
        />
      )}
      <p>
        {t(
          "A change is complete only after the cluster confirms its new state.",
        )}
      </p>
      {op.clusterID && (
        <Link
          className="button secondary"
          href={`/hosting/clusters/${op.clusterID}`}
        >
          {t("Back to cluster")}
        </Link>
      )}
    </section>
    </>
  );
}
