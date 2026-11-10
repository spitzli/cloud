"use client";
import { useModule } from "@spitzli/module-sdk/react";
import { mailNavigation } from "./navigation";
/** Derived per render: changing tenant or integration cannot retain another tenant's links. */
export function useMailNavigation(
  customerID: string,
  canManage: boolean,
  pathname: string,
  connection: string | null,
) {
  const mail = useModule("mail");
  return mail.available
    ? mailNavigation(customerID, canManage, pathname, connection)
    : [];
}
