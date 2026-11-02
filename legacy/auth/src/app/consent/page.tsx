import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { headers } from "next/headers";
import { redirect } from "next/navigation";
import { authView } from "@/lib/studio-client";
import { AuthPanel, ConsentForm } from "@/components/auth-forms";

export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Authorize access") }; }
export default async function Consent({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
 const { t } = await getRequestI18n();
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(await searchParams)) {
    for (const entry of Array.isArray(value)
      ? value
      : value === undefined
        ? []
        : [value])
      params.append(key, entry);
  }
  const invalid = (
    <AuthPanel title={t("Request unavailable")}>
      <p>
        {t("This authorization request is invalid or has expired. Return to the application and start signing in again.")}</p>
    </AuthPanel>
  );
  const view = await authView<{valid:boolean;session?:boolean;name?:string|null;scopes?:string[];claims?:string[]}>("/v1/auth/consent",{query:params.toString()},await headers());
  if (!view.valid) return invalid;
  if (!view.session) redirect(`/sign-in?${params}`);
  const name = view.name || t("Application");
  return (
    <ConsentForm
      name={name}
      scopes={view.scopes || []}
      claims={view.claims || []}
    />
  );
}
