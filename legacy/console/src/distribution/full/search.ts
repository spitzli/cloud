import { hostingCall } from "@/lib/hosting-client";
import { filterRows } from "@webdock/search";
import type { ConsoleSearchSources } from "@/lib/console-search";
import type { HostingAppView, HostingProjectContext, HostingPage, DatabaseView } from "@webdock/hosting-contracts";
export const productSearch: Partial<Pick<ConsoleSearchSources, "apps" | "databases" | "project" | "projects">> = {
  project: projectID => hostingCall<HostingProjectContext>({action:'projects.get',projectID}),
  apps: (projectID, search) => hostingCall<HostingPage<HostingAppView>>({action:'apps.list',projectID,search,page:1,limit:20,sort:'name'}),
  databases: projectID => hostingCall<DatabaseView[]>({action:'databases.list',projectID}),
  projects: async (customerID, query) => {
    const rows = await hostingCall<Array<{id:string;name:string}>>({action:'databases.projects.list',customerID});
    const matches = filterRows(rows,query,project=>`${project.name} ${project.id}`);
    return {docs:matches.slice(0,20).map(project=>({projectID:project.id,name:project.name})),totalDocs:matches.length};
  },
};
