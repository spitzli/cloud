import { headers } from "next/headers";
import { getStudioSession } from "@/lib/studio-client";
import { getTenant } from "@/lib/tenants";
import { AccessError } from "@/lib/access-management";
import { notFound, redirect } from 'next/navigation';
import { getRequestI18n } from '@webdock/i18n/next';
import { WorkspaceContext } from '@/components/workspace-context';
export default async function TenantLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ customerID: string }>;
}) {
  const { customerID } = await params;
  const { t } = await getRequestI18n();
  const session = await getStudioSession();
  if (!session) redirect('/api/sso/login?returnTo='+encodeURIComponent(`/tenants/${customerID}`));
  const requestHeaders = await headers();
  let data;
  try { data = await getTenant(requestHeaders, customerID); }
  catch (error) { if (error instanceof AccessError) notFound(); throw error; }
  return (
    <>
      <WorkspaceContext items={[{label:data.operator?t('Customers'):t('Workspaces'),href:data.operator?'/customers':'/tenants?choose=1'},{label:data.tenant.name,href:data.operator?`/customers/${customerID}`:undefined}]}/>
      {children}
    </>
  );
}
