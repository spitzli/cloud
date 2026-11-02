import {getStudioSession} from "@/lib/studio-client";
import {uiLabel} from "@/lib/ui-labels";

import { getRequestI18n } from '@webdock/i18n/next';
import { TenantPreviewStart } from "@/components/tenant-preview";
import { customerFields } from "../../../../../lib/customer-fields";
import Link from "next/link";
import { notFound } from "next/navigation";
import { Editor, ArchiveButton } from "../../../../../components/editor";
import { saveCustomer, archiveRecord } from "../../../../../lib/actions";
import { date, validRecordID } from "../../../../../lib/presentation";
import { requireRegistryOperator } from "../../../../../lib/registry-page";
export default async function Customer({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const installation = (await getStudioSession())?.installation;
  const available = (module:string) => ["enabled","draining"].includes(installation?.modules[module] ?? "");
  const i18n = await getRequestI18n();

  const { id } = await params;
  if (!validRecordID(id)) notFound();
  const { payload, user } = await requireRegistryOperator();
  const customer = await payload.findByID({
    collection: "customers",
    id,
    overrideAccess: false,
    user,
    disableErrors: true,
  });
  if (!customer) notFound();
  const projects = await payload.find({
    collection: "projects",
    where: { customer: { equals: id } },
    limit: 8,
    sort: "-updatedAt",
    user,
    overrideAccess: false,
    depth: 0,
  });
  return (
    <>
      <Link className="back" href="/customers">{i18n.t("← Customers")}</Link>
      <div className="page-heading">
        <div>
          <h1>{customer.name}</h1>
          <p>
            <span className="badge">{i18n.t(uiLabel(customer.status))}</span>
          </p>
        </div>
        {customer.status === "active" && (
          <Link href={"/projects/new?customer=" + id} className="button">{i18n.t("Add project +")}</Link>
        )}
      </div>
      <div className="customer-overview">
        <div>
          <section className="panel">
            <div className="section-heading">
              <h2>{i18n.t("Projects (")}{projects.totalDocs})</h2>
              <Link className="small-link" href={"/?status=all&customer=" + id}>{i18n.t("View all")}</Link>
            </div>
            {projects.docs.map((p) => (
              <Link
                className="related-project"
                key={p.id}
                href={"/projects/" + p.id}
              >
                {p.name}
                <span
                  className={
                    "badge " + (p.status === "active" ? "connected" : "")
                  }
                >
                  {i18n.t(uiLabel(p.status))}
                </span>
              </Link>
            ))}
            {!projects.totalDocs && <p className="muted">{i18n.t("No projects yet.")}</p>}
          </section>
          <section className="account-section">
            <h2>{i18n.t("Services & team")}</h2>
            {customer.status === "active" && <TenantPreviewStart customerID={id}/>}

            <div className="customer-tenant-actions">
            <Link className="button" href={`/tenants/${id}/team`}>{i18n.t("Team & invitations")}</Link>
            {available("hosting") && <Link className="button secondary" href={`/tenants/${id}/hosting`}>{i18n.t("Hosting")}</Link>}
            {available("mail") && <Link className="button secondary" href={`/tenants/${id}/mail`}>{i18n.t("Mail & domains")}</Link>}
            </div>
          </section>
        <details className="workspace-disclosure">
          <summary>{i18n.t("Edit customer details")}</summary>
          <Editor
            action={saveCustomer.bind(null, id)}
            fields={customerFields(customer)}
          />
        </details>
          <section className="record-meta">
            <p>{i18n.t("Customer ID ")}<code>{id}</code>
              <small>{i18n.t("Created ")}{date(customer.createdAt,i18n.locale)}{i18n.t(" · Updated")}{" "}
                {date(customer.updatedAt,i18n.locale)}
              </small>
            </p>
            <p>
              <Link
                className="small-link"
                href={"/activity?collection=customers&target=" + id}
              >{i18n.t("View customer history")}</Link>
            </p>
            <ArchiveButton
              action={archiveRecord.bind(null, "customers", id)}
              archived={customer.status === "archived"}
            />
          </section>
        </div>
      </div>
    </>
  );
}
