"use client";
import { usePathname, useSearchParams } from "next/navigation";
import { useI18n } from "@spitzli/i18n/react";
import { useMailNavigation } from "@spitzli/module-mail/hooks";
import type { ReactNode } from "react";
import type { mailNavigation } from "../navigation";
export function MailNavigation({
  customerID,
  canManage,
  renderNavigation,
}: {
  customerID: string;
  canManage: boolean;
  renderNavigation: (props: {
    label: string;
    links: ReturnType<typeof mailNavigation>;
  }) => ReactNode;
}) {
  const i18n = useI18n(),
    pathname = usePathname(),
    query = useSearchParams();
  const links = useMailNavigation(
    customerID,
    canManage,
    pathname,
    query.get("connection"),
  );
  return links.length
    ? renderNavigation({ label: i18n.t("Mail areas"), links })
    : null;
}
