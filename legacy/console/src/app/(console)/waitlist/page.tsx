import { Waitlist } from "@clerk/nextjs";
import { notFound } from "next/navigation";
import { clerkConsoleEnabled } from "@/lib/console-identity";
import { ConsoleAuthShell, consoleAuthAppearance } from "@/components/console-auth-shell";
export default function WaitlistPage() {
  if (!clerkConsoleEnabled()) notFound();
  return <ConsoleAuthShell title="Get early access" description="Webdock is currently invitation-only. Leave your email and we will let you know when access is available.">
    <Waitlist signInUrl="/login" afterJoinWaitlistUrl="/waitlist/confirmed" appearance={{...consoleAuthAppearance, elements: {...consoleAuthAppearance.elements, header: {display: "none"}}}} />
  </ConsoleAuthShell>;
}
