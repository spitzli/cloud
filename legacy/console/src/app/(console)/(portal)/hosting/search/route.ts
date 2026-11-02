import { searchConsole, findConsoleProjects } from '@/lib/console-search-server';
const headers = { 'Cache-Control': 'private, no-store', Vary: 'Cookie' };
export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  if ([...params.keys()].some(key => !['kind','query','customerID','projectID'].includes(key) || params.getAll(key).length !== 1)) {
    return Response.json({results:[],error:'Invalid search.'},{status:400,headers});
  }
  const query = params.get('query') || '';
  const customerID = params.get('customerID') || undefined;
  if (params.get('kind') === 'projects') return Response.json(await findConsoleProjects(customerID || '',query),{headers});
  if (params.has('kind')) return Response.json({results:[],error:'Invalid search.'},{status:400,headers});
  return Response.json(await searchConsole({query,customerID,projectID:params.get('projectID')||undefined}),{headers});
}
