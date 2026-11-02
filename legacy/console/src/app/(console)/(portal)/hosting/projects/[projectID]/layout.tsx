import { HostingProjectHeader } from '@/components/hosting/project-context';
export default async function HostingProjectLayout({ children, params }: { children: React.ReactNode; params: Promise<{projectID:string}> }) {
  const { projectID } = await params;
  return <><HostingProjectHeader projectID={projectID} showNavigation={false}/>{children}</>;
}
