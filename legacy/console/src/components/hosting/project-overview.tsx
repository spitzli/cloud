import type { ComponentProps } from "react";
import { getProjectOverview } from "@/lib/project-overview";
import { applicationState } from "@/lib/workspace-navigation";
import { createProjectOverviewView } from "@webdock/module-hosting/ui/project-overview";
const { ProjectOverview: ProjectOverviewView } = createProjectOverviewView({
  getProjectOverview,
  applicationState,
});
export async function ProjectOverview(
  props: ComponentProps<typeof ProjectOverviewView>,
) {
  return <ProjectOverviewView {...props} />;
}
