import {requireRegistryOperator} from "@/lib/registry-page";
import { randomUUID } from "node:crypto";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingTable } from "@/components/hosting/table";
import { Pagination, ListControls } from "@/components/list-controls";
import type {
  HostingPage,
  HostingClusterView,
} from "@webdock/hosting-contracts";
export default async function Infrastructure({
  searchParams,
}: {
  searchParams: Promise<{ q?: string; page?: string; sort?: string }>;
}) {
  const { t } = await getRequestI18n();
  const params = await searchParams;
  const page = /^[1-9][0-9]{0,3}$/.test(params.page ?? "")
    ? Number(params.page)
    : 1;
  const sort = params.sort === "-name" ? "-name" : "name";
  const data = await hostingPageCall<HostingPage<HostingClusterView>>(
    {
      action: "clusters.list",
      page,
      limit: 20,
      sort,
      search: (params.q ?? "").slice(0, 160),
    },
    "/infrastructure",
  );
  const {payload,user}=await requireRegistryOperator();
  const customers=await payload.find({collection:"customers",where:{status:{equals:"active"}},limit:100,sort:"name",depth:0,user,overrideAccess:false});
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">{t("Hosting")}</p>
          <h1>{t("Infrastructure")}</h1>
          <p>
            {t("Manage independent k3s installations and multi-node clusters.")}
          </p>
        </div>
      </div>
      <section className="panel">
        <ListControls
          path="/infrastructure"
          q={params.q ?? ""}
          sort={sort}
          placeholder={t("Search")}
          sortOptions={[
            { value: "name", label: "Name A–Z" },
            { value: "-name", label: "Name Z–A" },
          ]}
        />
        <HostingTable
          rows={data.docs}
          rowKey={(r) => r.id}
          empty={t("No hosting clusters connected yet.")}
          columns={[
            {
              label: t("Cluster"),
              render: (r) => (
                <Link href={r.ownership==='tenant'?`/hosting/clusters/${r.id}`:`/infrastructure/${r.id}`}>{r.name}</Link>
              ),
            },
            {
              label: t("Location"),
              render: (r) =>
                `${r.region}, ${r.country ?? t("Country unverified")}`,
            },
            {label:t("Managed by"),render:r=>r.ownership==='tenant'?t("Customer-owned cluster"):"Webdock"},
            { label: t("Status"), render: (r) => t(r.state) },
            {
              label: t("Nodes"),
              render: (r) => r.observation?.nodes.length ?? "—",
            },
          ]}
        />
        <Pagination
          path="/infrastructure"
          query={{ q: params.q, sort }}
          {...data}
        />
      </section>
      <section className="panel">
        <h2>{t("Register a cluster")}</h2>
        <p>
          {t(
            "Registration creates inventory only. Your existing server is not modified.",
          )}
        </p>
        <HostingForm
          command={{
            action: "clusters.register",
            name: "",
            provider: "",
            country: null,
            region: "",
            locationEvidence: "",
            idempotencyKey: randomUUID(),
          }}
          label="Register cluster"
        >
          {(["name", "provider", "region", "locationEvidence"] as const).map(
            (field, i) => (
              <label className="field" key={field}>
                {
                  [
                    t("Name"),
                    t("Infrastructure provider"),
                    t("Region"),
                    t("Location evidence"),
                  ][i]
                }
                <input
                  name={field}
                  required
                  maxLength={field === "locationEvidence" ? 2000 : 160}
                />
              </label>
            ),
          )}
          <label className="field">
            {t("EU country code (optional)")}
            <input
              name="country"
              defaultValue=""
              pattern="[A-Z]{2}"
              maxLength={2}
            />
          </label>
          <label className="field">{t("Dedicated customer")}<select name="dedicatedCustomerID"><option value="">{t("Shared platform cluster")}</option>{customers.docs.map(c=><option key={c.id} value={String(c.id)}>{c.name}</option>)}</select></label>
        </HostingForm>
      </section>
    </>
  );
}
