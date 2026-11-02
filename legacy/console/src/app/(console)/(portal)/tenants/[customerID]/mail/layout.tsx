import { getStudioSession } from "@/lib/studio-client";
import {notFound} from 'next/navigation';
import {AccessError} from '@/lib/studio-errors';
import { getTenant } from '@/lib/tenants';
import { headers } from 'next/headers';
export default async function MailLayout({children,params}: {children:React.ReactNode;params:Promise<{customerID:string}>}) {
  const installation = (await getStudioSession())?.installation;
  if (!installation || installation.modules.mail === "disabled") notFound();
  const {customerID}=await params;
  await getTenant(await headers(),customerID).catch(error=>{if(error instanceof AccessError)notFound();throw error;});
  return <>{children}</>;
}
