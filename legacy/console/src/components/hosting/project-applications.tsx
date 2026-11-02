import type { ComponentProps } from "react";
import { hostingCall } from "@/lib/hosting-client";
import { applicationState } from "@/lib/workspace-navigation";
import { createProjectApplicationsView } from "@webdock/module-hosting/ui/project-applications";
const { ProjectApplications: ProjectApplicationsView } =
  createProjectApplicationsView({ hostingCall, applicationState });
import { getStudioSession } from "@/lib/studio-client";
export async function ProjectApplications(
  props: ComponentProps<typeof ProjectApplicationsView>,
) {
  if (
    !["enabled", "draining"].includes(
      (await getStudioSession())?.installation?.modules.hosting ?? "",
    )
  )
    return null;
  return <ProjectApplicationsView {...props} />;
}
