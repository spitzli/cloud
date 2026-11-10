import * as React from "react";
import { getRequestI18n } from "@spitzli/i18n/next";
import type { DatabaseUIServices } from "./services";
export function createTenantDatabaseNavigationView(
  services: Pick<DatabaseUIServices, "getStudioSession" | "AreaNavigation">,
) {
  const { getStudioSession, AreaNavigation } = services;
  async function TenantDatabaseNavigation({
    customerID,
  }: {
    customerID: string;
  }) {
    const { t } = await getRequestI18n();
    const session = await getStudioSession();
    const base = `/tenants/${customerID}`;
    return (
      <AreaNavigation
        label={t("Database areas")}
        links={[
          ...(!session?.preview
            ? [{ href: base + "/databases", label: t("Browse data") }]
            : []),
          { href: base + "/managed-databases", label: t("Managed PostgreSQL") },
        ]}
      />
    );
  }
  return { TenantDatabaseNavigation };
}
