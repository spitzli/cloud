import type { ComponentProps } from "react";
import { getStudioSession } from "@/lib/studio-client";
import { AreaNavigation } from "@/components/area-navigation";
import { createTenantDatabaseNavigationView } from "@webdock/module-databases/ui/tenant-database-navigation";
const { TenantDatabaseNavigation: TenantDatabaseNavigationView } =
  createTenantDatabaseNavigationView({ getStudioSession, AreaNavigation });
export async function TenantDatabaseNavigation(
  props: ComponentProps<typeof TenantDatabaseNavigationView>,
) {
  return <TenantDatabaseNavigationView {...props} />;
}
