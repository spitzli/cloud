import type { ComponentProps } from "react";
import { HostingActionBoundary } from "./action-boundary";
import { hostingUIScope } from "@spitzli/module-hosting/ui/scope";
import { hostingPageCall } from "@/lib/hosting-client";
import { createOwnMailView } from "@spitzli/module-hosting/ui/own-mail";
const { OwnMail: OwnMailView } = createOwnMailView({ hostingPageCall });
export async function OwnMail(props: ComponentProps<typeof OwnMailView>) {
  return (
    <HostingActionBoundary key={hostingUIScope(props)}>
      <OwnMailView {...props} />
    </HostingActionBoundary>
  );
}
