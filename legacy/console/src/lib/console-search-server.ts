import {browserAuthConfiguration} from "./browser-auth-config";
import {moduleForPath} from "@webdock/module-sdk/selection";
import {hasProductModules} from "./module-data";
import {relatedID} from "./presentation";
import 'server-only';
import { filterRows } from '@webdock/search';
import { headers } from 'next/headers';
import { getRequestI18n } from '@webdock/i18n/next';
import { getStudioSession } from './studio-client';
import { getTenant, listTenants } from './tenants';
import { productSearch } from "@distribution/search";
import { registryClient } from './registry-client';
import { queryConsole, type ConsoleSearchInput, type ConsoleSearchResponse, type ConsoleResult } from './console-search';


export async function searchConsole(input: ConsoleSearchInput): Promise<ConsoleSearchResponse> {
  const requestHeaders = await headers(), { t } = await getRequestI18n();
  try {
    const session = await getStudioSession();
    const available=(id:string)=>["enabled","draining"].includes(session?.installation?.modules[id] ?? "");
    const result = await queryConsole(input, {
      session: getStudioSession,
      tenant: id => getTenant(requestHeaders, id),
      workspaces: () => listTenants(requestHeaders),
      sites: () => Promise.resolve([]),
      bridges: () => Promise.resolve({}),
      project: async projectID => session?.operator ? {customerID:relatedID((await registryClient.findByID({collection:'projects',id:projectID,depth:0})).customer)} : (productSearch.project ? productSearch.project(projectID) : Promise.reject(Error("Project catalog unavailable"))),
      projects: searchWorkspaceProjects,
      allProjects: query => registryClient.find({collection:'projects',where:{and:[{status:{equals:'active'}},{name:{contains:query}}]},limit:20,depth:0,sort:'name'}),
      apps: (projectID, search) => available('hosting') && productSearch.apps ? productSearch.apps(projectID,search) : Promise.resolve({docs:[],totalDocs:0}),
      databases: projectID => available('databases') && productSearch.databases ? productSearch.databases(projectID) : Promise.resolve([]),
      navigation: ({customerID,projectID}, canManage, operator, preview) => {
        const result: ConsoleResult[] = [
          {id:'nav:workspaces',kind:'navigation',label:t('Workspaces'),href:'/tenants?choose=1'},
          ...(!preview ? [{id:'nav:developer',kind:'navigation' as const,label:t('Developer tools'),description:t('API & MCP'),href:'/hosting/developer'}] : []),
        ];
        if (customerID) {
          const base = `/tenants/${customerID}`;
          result.push(...[
            [t('Overview'),base], [t('Applications'),base+'/hosting'], [t('Databases'),base+(preview?'/managed-databases':'/databases')],
            [t('Plan & usage'),base+'/usage'], [t('Profile'),base+'/profile'],
            ...(!preview ? [[t('Mail'),base+'/mail'],[t('Mailboxes'),base+'/mail/mailboxes'],[t('Quickstart'),base+'/mail/quickstart']] : []),
            ...(canManage ? [[t('Team'),base+'/team']] : []),
          ].map(([label,href])=>({id:'nav:'+href,label,href,kind:'navigation' as const})));
        }
        if(projectID) result.push({id:'nav:deployments',kind:'navigation',label:t('Deployments'),href:`/hosting/projects/${projectID}/git`});
        if(operator) result.push(...[[t('Projects'),'/'],...(browserAuthConfiguration.provider==='clerk'?[[t('Studio accounts'),'/admin/registry-users']]:[[t('People & access'),'/people']]),[t('Plans'),'/admin/plans'],[t('Connections'),'/integrations'],[t('Settings'),'/admin']].map(([label,href])=>({id:'nav:'+href,label,href,kind:'navigation' as const})));
        return result.filter(item=>{const module=moduleForPath(item.href);return (!module || available(module)) && (hasProductModules(session?.installation) || !/\/(usage|plans)$/.test(item.href));});
      },
    });
    return {...result, error:result.error ? t(result.error) : undefined};
  } catch { return {results:[],error:t('Search is temporarily unavailable. Please try again.')}; }
}
export async function findConsoleProjects(customerID: string, query: string) {
  const session = await getStudioSession();
  if (!session || session.preview?.status === 'expired' || typeof customerID !== 'string' || !/^[1-9][0-9]{0,18}$/.test(customerID) || typeof query !== 'string' || query.length > 160) return {projects:[],error:true};
  try {
    await getTenant(await headers(),customerID);
    const page=await searchWorkspaceProjects(customerID,query);
    return {projects:page.docs.map(project=>({id:project.projectID,name:project.name})),more:page.totalDocs>page.docs.length};
  } catch { return {projects:[],error:true}; }
}

/** The workspace catalog includes projects without a container-hosting assignment. */
async function searchWorkspaceProjects(customerID: string, query: string) {
  if ((await getStudioSession())?.operator) {
    const page=await registryClient.find({collection:'projects',where:{and:[{customer:{equals:customerID}},{status:{equals:'active'}},{name:{contains:query}}]},limit:20,depth:0,sort:'name'});
    return {docs:page.docs.map(p=>({projectID:String(p.id),name:p.name})),totalDocs:page.totalDocs};
  }
  return productSearch.projects ? productSearch.projects(customerID,query) : {docs:[],totalDocs:0};
}
