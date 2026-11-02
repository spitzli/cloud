import { auth } from "@clerk/nextjs/server";
import { getStudioSession } from "@/lib/studio-client";
import { SelectedAuthControls } from "@/components/selected-auth-controls";
import { SignIn } from "@clerk/nextjs";
import { redirect } from "next/navigation";
import {
  browserAuthConfiguration,
  safeBrowserReturn,
} from "@/lib/browser-auth-config";
export default async function SignInPage({
  searchParams,
}: {
  searchParams: Promise<{ returnTo?: string }>;
}) {
  if (browserAuthConfiguration.provider !== "clerk") redirect("/api/sso/login");
  const { returnTo } = await searchParams;
  if ((await auth()).userId) {
    if (await getStudioSession()) redirect(safeBrowserReturn(returnTo));
    return (
      <main id="main" className="access-page">
        <section className="auth-panel">
          <h1>Finish account setup</h1>
          <p>
            Your sign-in is verified, but this account is not enabled in this
            installation. Ask an administrator to finish linking your account
            and security setup.
          </p>
          <a href="/account">Account and security</a>
          <SelectedAuthControls />
        </section>
      </main>
    );
  }
  return (
    <main id="main" className="access-page">
      <SignIn
        routing="path"
        path="/sign-in"
        forceRedirectUrl={safeBrowserReturn(returnTo)}
        withSignUp={false}
      />
    </main>
  );
}
