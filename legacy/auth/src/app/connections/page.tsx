
import {withAuthenticatedDatabase} from '@/lib/database-context';
import { getRequestI18n } from "@webdock/i18n/next";
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import Link from "next/link";
import { auth } from "@/lib/auth";
import { currentMCPClaims, mcpResource } from "@/lib/mcp";
import { studioCall } from "@/lib/studio-client";
import { ClientForm } from "./form";
import { disableClient } from "./actions";
export default async function Connections() {

 const { t } = await getRequestI18n();
  const session = await auth.api.getSession({ headers: await headers() });
  if (!session) redirect("/sign-in");
  if ((await currentMCPClaims(session.user.id)).disabled) redirect("/account");
  return withAuthenticatedDatabase(await headers(), async () => {
  const rows = await studioCall<{clientId:string;name:string;disabled:boolean}[]>("getConnections");
  return <section className="auth-panel"><p className="eyebrow">{t("Operator connections")}</p><h1>{t("Connect your tools")}</h1><p>{t("Let an MCP client read your Webdock registry or manage records with your permission.")}</p><p>{t("Server URL: ")}<code>{mcpResource}</code></p><ClientForm /><h2>{t("Registered clients")}</h2>{rows.length ? rows.map(row => <article key={row.clientId}><h3>{row.name || t("MCP client")}</h3><p><code>{row.clientId}</code> · {row.disabled ? t("Disabled") : t("Active")}</p>{!row.disabled && <form action={disableClient}><input type="hidden" name="id" value={row.clientId} /><button className="button secondary">{t("Disable access")}</button></form>}</article>) : <p>{t("No MCP clients registered yet.")}</p>}<Link href="/account">{t("Back to account")}</Link></section>;

});
}
