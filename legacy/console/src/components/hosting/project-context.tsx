import type { ComponentProps } from "react";
import { getHostingProject } from "@/lib/hosting-project";
import { AreaNavigation } from "../area-navigation";
import { createProjectContextView } from "@webdock/module-hosting/ui/project-context";
const { HostingProjectHeader: HostingProjectHeaderView } =
  createProjectContextView({ getHostingProject, AreaNavigation });
export async function HostingProjectHeader(
  props: ComponentProps<typeof HostingProjectHeaderView>,
) {
  return <HostingProjectHeaderView {...props} />;
}
