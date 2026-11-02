import { AccessError } from "@/lib/studio-errors";
import { headers } from "next/headers";
import { notFound, redirect } from "next/navigation";
import { getRequestI18n } from "@webdock/i18n/next";
import { getStudioSession } from "@/lib/studio-client";
import { getTenant } from "@/lib/tenants";
import {
  mailRead as mailCall,
  type MailConnection,
  type MailRow,
} from "@/lib/mail-product";
export type MailPageProps = {
  params: Promise<{ customerID: string; id?: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};
export async function loadMailPage(props: MailPageProps, path: string) {
  const params = await props.params,
    query = await props.searchParams;
  const session = await getStudioSession();
  const base = `/tenants/${params.customerID}/mail`;
  if (!session) {
    const search = new URLSearchParams(
      Object.entries(query).filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    );
    redirect(
      "/api/sso/login?returnTo=" +
        encodeURIComponent(
          base + path + (search.size ? "?" + search.toString() : ""),
        ),
    );
  }
  if (session.preview) notFound();
  const [tenant, connections, i18n] = await Promise.all([
    getTenant(await headers(), params.customerID).catch((error) => {
      if (error instanceof AccessError) notFound();
      throw error;
    }),
    mailCall<MailConnection[]>("mailConnections", params.customerID),
    getRequestI18n(),
  ]);
  if (query.connection !== undefined && typeof query.connection !== "string")
    notFound();
  const connection = query.connection
    ? connections.find((c) => c.id === query.connection)
    : connections[0];
  if (query.connection && !connection) notFound();
  return {
    customerID: params.customerID,
    id: params.id,
    query,
    base,
    connection,
    connections,
    canManage: tenant.canManage,
    operator: tenant.operator,
    tenantName: tenant.tenant.name,
    tenantData: tenant,
    i18n,
  };
}
export type MailPageContext = Awaited<ReturnType<typeof loadMailPage>>;

export {
  mailHref,
  MailPageHeading,
  MailNoConnection,
  stateLabels,
  MailRows,
} from "@webdock/module-mail/ui/product-page";
