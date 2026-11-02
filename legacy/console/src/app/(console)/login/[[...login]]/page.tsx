import { SignIn } from "@clerk/nextjs";
import { redirect } from "next/navigation";
import { clerkConsoleEnabled } from "@/lib/console-identity";
import { ConsoleAuthShell, consoleAuthAppearance } from "@/components/console-auth-shell";
export const dynamic = "force-dynamic";
export default function Login() {
  if (!clerkConsoleEnabled()) redirect("/api/sso/login");
  return <ConsoleAuthShell showIntro={false} title="Welcome back" description="Sign in to manage your workspaces and projects.">
    <SignIn routing="path" path="/login" signUpUrl="/waitlist" appearance={consoleAuthAppearance} />
  </ConsoleAuthShell>;
}
