import Link from "next/link";
import {redirect} from "next/navigation";
import {formatResource} from "@webdock/hosting-contracts";
import { HostingAllowanceFields,HostingAllowanceList } from "@/components/hosting/allowances";
import { normalizeHostingAllowances } from "@webdock/hosting-contracts";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingTable } from "@/components/hosting/table";
import type { HostingClusterView } from "@webdock/hosting-contracts";
export default async function Cluster({
  params,
}: {
  params: Promise<{ clusterID: string }>;
}) {
  const { clusterID } = await params;
  const { t,locale,date } = await getRequestI18n();
  const c = await hostingPageCall<HostingClusterView>(
    {
      action: "clusters.get",
      clusterID,
    },
    `/infrastructure/${encodeURIComponent(clusterID)}`,
  );
  if(c.ownership==='tenant')redirect(`/hosting/clusters/${clusterID}`);
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{c.name}</h1>
          <Link className="button secondary" href={`/infrastructure/${clusterID}/databases`}>{t("PostgreSQL operator management")}</Link>
          <p>
            {c.provider} · {c.region}, {c.country ?? t("Country unverified")}
          </p>
        </div>
      </div>
      <section className="panel">
        <h2>{t("Cluster status")}</h2>
        <dl className="allowance-list">
          <div>
            <dt>{t("Connection")}</dt>
            <dd>{t(c.state)}</dd>
          </div>
          <div>
            <dt>{t("Last observation")}</dt>
            <dd>{c.lastSeen?date(c.lastSeen,{dateStyle:"medium",timeStyle:"short"}):t("Unavailable")}</dd>
          </div>
          <div>
            <dt>{t("Workload readiness")}</dt>
            <dd>{c.workloadReady ? t("Verified") : t("Not verified")}</dd>
          </div>
          <div>
            <dt>{t("Location evidence")}</dt>
            <dd>{c.locationEvidence}</dd>
          </div>
        </dl>
        <p>
          {t(
            "A connected agent does not prove workload isolation, storage or TLS readiness.",
          )}
        </p>
      </section>
      <section className="panel">
        <h2>{t("Nodes")}</h2>
        <HostingTable
          rows={c.observation?.nodes ?? []}
          rowKey={(r) => r.id}
          empty={t("No node observations available.")}
          columns={[
            { label: t("Name"), render: (r) => r.id },
            { label: t("Role"), render: (r) => r.role==='server'?t("Control plane"):t("Worker") },
            { label: t("Architecture"), render: (r) => r.architecture },
            { label: t("CPU cores"), render: (r) => formatResource("cpuMillicores",r.cpuMillicores,locale) },
            {label:t("Memory"),render:(r)=>formatResource("memoryBytes",r.memoryBytes,locale)},
          ]}
        />
      </section>
      {c.capacity&&<section className="panel"><h2>{t("Capacity for applications")}</h2><HostingAllowanceList values={c.capacity}/></section>}
      <details className="panel" open={!c.verified}><summary>{t("Capacity and security settings")}</summary>
        <p>
          {t(
            "Reserve platform capacity and record the verified security baseline. Persistent storage requires the private storage helper and verified filesystem capacity.",
          )}
        </p>
        <HostingForm
          preserveRevision
          command={{
            action: "clusters.activate",
            clusterID,
            revision: c.revision,
            capacity: normalizeHostingAllowances(c.capacity ?? undefined),
            verificationEvidence: "",
          }}
          label={c.verified?t("Update cluster capacity"):t("Enable managed execution")}
        >
          <HostingAllowanceFields
            extras
            values={c.capacity ?? normalizeHostingAllowances(undefined)}
          />
          <label className="field">
            {t("Security verification evidence")}
            <textarea
              name="verificationEvidence"
              required
              minLength={40}
              maxLength={3000}
            />
          </label>
        </HostingForm>
      </details>
      <details className="panel" open={c.state==='unconnected'}>
        <summary>{t("Agent connection")}</summary>
        <HostingForm
          command={{ action: "clusters.enrollment", clusterID }}
          label="Create enrollment"
        />
        <p>
          {t(
            "Revoking the agent disconnects management. Existing workloads are not deleted.",
          )}
        </p>
        <HostingForm
          command={{
            action: "clusters.revoke",
            clusterID,
            revision: c.revision,
          }}
          label="Revoke agent"
        />
      </details>
    </>
  );
}
