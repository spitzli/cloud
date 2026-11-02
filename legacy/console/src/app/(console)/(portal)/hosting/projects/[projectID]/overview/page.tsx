import { ProjectOverview } from "@/components/hosting/project-overview";
export default async function Page({
  params,
}: {
  params: Promise<{ projectID: string }>;
}) {
  const { projectID } = await params;
  return <ProjectOverview projectID={projectID} />;
}
