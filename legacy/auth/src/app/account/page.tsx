import { getRequestI18n } from "@webdock/i18n/next";
import type { Metadata } from "next";
import { Account } from "@/components/auth-forms";
import { businessURL, studioURL } from "@/lib/studio-links";
export async function generateMetadata(): Promise<Metadata> { const { t } = await getRequestI18n(); return { title: t("Your account") }; }
export default function AccountPage() {
  return <Account links={{ tenants: businessURL("/tenants"), sites: businessURL("/sites"), people: businessURL("/people"), admin: businessURL("/admin"), studio: studioURL() }} />;
}
