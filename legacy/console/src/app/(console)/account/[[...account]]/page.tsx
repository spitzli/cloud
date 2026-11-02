import { auth } from "@clerk/nextjs/server";
import { UserProfile } from "@clerk/nextjs";
import { redirect } from "next/navigation";
import { browserAuthConfiguration } from "@/lib/browser-auth-config";
export default async function AccountPage() {
  if (browserAuthConfiguration.provider !== "clerk")
    redirect(
      new URL(
        "/account",
        process.env.WEBDOCK_AUTH_ISSUER || "https://auth.webdock.dev/api/auth",
      ).href,
    );
  await auth.protect();
  return (
    <main id="main" className="access-page">
      <UserProfile routing="path" path="/account" />
    </main>
  );
}
