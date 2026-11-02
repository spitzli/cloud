import Link from "next/link";
import { notFound } from "next/navigation";
import { headers } from "next/headers";
import { getRequestI18n } from "@webdock/i18n/next";
import { getTenantMailboxes } from "@/lib/mailboxes";
import { Webmail } from "@/components/mail/webmail";
import "./style.css";

export default async function Inbox({ params, searchParams }: { params: Promise<{ customerID: string }>; searchParams: Promise<{ mailbox?: string }> }) {
  const { customerID } = await params, { mailbox } = await searchParams, { t } = await getRequestI18n();
  const data = await getTenantMailboxes(await headers(), customerID);
  const selected = data?.mailboxes.find(box=>box.id===mailbox && box.canOpen);
  if (!selected) notFound();
  return <div className="webmail-page"><header className="page-heading"><div><Link href={`/tenants/${customerID}/mail/mailboxes`}>{t("Mailboxes")}</Link><h1>{selected.name}</h1><p className="muted">{selected.address}</p></div></header><Webmail key={selected.id} customerID={customerID} mailboxID={selected.id} address={selected.address} /></div>;
}
