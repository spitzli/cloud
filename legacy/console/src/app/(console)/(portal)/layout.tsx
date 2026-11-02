
import { getRequestI18n } from '@webdock/i18n/next';
import { headers } from "next/headers";
import { auth } from "@/lib/auth";
import { StudioShell } from "@/components/studio-shell";
import {clerkConsoleEnabled} from "@/lib/console-identity";
import {auth as clerkAuth} from "@clerk/nextjs/server";
export const dynamic = "force-dynamic";
export default async function PortalLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  if (clerkConsoleEnabled()) await clerkAuth.protect();
  const i18n = await getRequestI18n();

  const session = await auth.api.getSession({ headers: await headers() });
  return (
    <StudioShell preview={session?.preview}>
      <div className="studio-portal">{session?.preview?.status === "expired" ? <section className="auth-panel"><h1>{i18n.t("Preview ended")}</h1><p>{i18n.t("Return to your own view using the banner to continue.")}</p></section> : children}</div>
    </StudioShell>
  );
}
