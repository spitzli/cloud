"use client";
import { MailNavigation as Navigation } from "@spitzli/module-mail/ui/mail-navigation";
import { AreaNavigation } from "@/components/area-navigation";
export function MailNavigation(props: {
  customerID: string;
  canManage: boolean;
}) {
  return (
    <Navigation
      {...props}
      renderNavigation={(navigation) => <AreaNavigation {...navigation} />}
    />
  );
}
