import { notFound } from "next/navigation";
import { getStudioSession } from "@/lib/studio-client";
import { getRequestI18n } from "@webdock/i18n/next";
import type { DatabaseBinding } from "@webdock/database-contracts";
import { databaseCall } from "@/lib/database-client";
import { withHostingPageAccess } from "@/lib/hosting-page-access";
import { DatabaseWorkbench } from "@/components/databases/workbench";
import { DatabaseWorkspaceFrame } from "@/components/databases/workspace-frame";
import "../style.css";

export default async function Database({ params }: { params: Promise<{ customerID: string; bindingID: string }> }) {
  const { customerID, bindingID } = await params, { t } = await getRequestI18n();
  const base = `/tenants/${customerID}/databases`;
  const binding = await withHostingPageAccess(`${base}/${bindingID}`, () => databaseCall<DatabaseBinding>({ action: "get", bindingID }));
  const session = await getStudioSession();
  if (binding.customerID !== customerID || !session) notFound();
  return <DatabaseWorkspaceFrame key={bindingID} name={binding.name} back={base}
    environment={t(binding.environment === "production" ? "Production" : binding.environment === "staging" ? "Staging" : "Development")}
    permission={t(binding.profile === "read" ? "Read only" : binding.profile === "write" ? "Edit data" : "Manage schema")}>
    <DatabaseWorkbench key={`${session.user.id}:${bindingID}`} bindingID={bindingID} userID={session.user.id} />
  </DatabaseWorkspaceFrame>;
}
