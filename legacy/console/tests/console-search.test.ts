import test from 'node:test';
import assert from 'node:assert/strict';
import { queryConsole, type ConsoleSearchSources } from '../src/lib/console-search';
import type { TenantView } from '../src/lib/studio-contracts';

const tenant = { tenant:{id:'1',name:'North',status:'active'},canManage:false,operator:false,sites:[{id:'11',name:'Store',url:'https://store.example.com',role:'reader'}],members:[],invitations:[] } as unknown as TenantView;
function sources(overrides: Partial<ConsoleSearchSources> = {}): ConsoleSearchSources {
  return {
    session:async()=>({operator:false}),tenant:async()=>tenant,
    workspaces:async()=>[{id:'1',name:'North',status:'active',role:'member'}],
    sites:async()=>tenant.sites,bridges:async()=>({'11':{}}),project:async()=>({customerID:'1'}),
    projects:async()=>({docs:[{projectID:'2',name:'Store frontend'}],totalDocs:1}),allProjects:async()=>({docs:[],totalDocs:0}),
    apps:async()=>({docs:[{id:'3',name:'Store app',status:'ready'}],totalDocs:1}),databases:async()=>[{id:'4',name:'Store data',state:'ready'}],
    navigation:()=>[],...overrides,
  };
}
test('unauthenticated and expired previews cannot enumerate resources',async()=>{
  const forbidden=async()=>{throw Error('Resource enumeration must not run');};
  for(const session of [null,{operator:false,preview:{status:'expired'}}]) {
    const result=await queryConsole({query:'Store'},sources({session:async()=>session,sites:forbidden,workspaces:forbidden}));
    assert.deepEqual(result.results,[]);assert.ok(result.error);
  }
});
test('a forged project/workspace combination fails before loading resource results',async()=>{
  let loaded=0;
  const result=await queryConsole({query:'Store',customerID:'9',projectID:'2'},sources({tenant:async()=>{loaded++;return tenant;},apps:async()=>{loaded++;return{docs:[],totalDocs:0};}}));
  assert.equal(loaded,0);assert.deepEqual(result.results,[]);assert.match(result.error!,/not in/);
});
test('workspace searches use the authorized tenant websites and never the global site directory',async()=>{
  let globalSites=0;
  const result=await queryConsole({query:'Store',customerID:'1'},sources({sites:async()=>{globalSites++;return [{id:'99',name:'Other tenant secret',url:'https://other.example',role:'admin'}];}}));
  assert.equal(globalSites,0);assert.equal(result.results.some(r=>r.href==='/sites/11'),true);assert.doesNotMatch(JSON.stringify(result),/Other tenant|99/);
});
test('partial service failures retain useful results and explicitly report incomplete search',async()=>{
  const result=await queryConsole({query:'Store',projectID:'2'},sources({apps:async()=>{throw Error('private backend failure');}}));
  assert.equal(result.partial,true);assert.ok(result.results.some(r=>r.kind==='database'));
  assert.doesNotMatch(JSON.stringify(result),/private backend failure/);
});
test('preview search does not offer external CMS destinations and invalid URL schemes never render',async()=>{
  const result=await queryConsole({query:'Store'},sources({session:async()=>({operator:false,preview:{status:'active'}}),bridges:async()=>({}),sites:async()=>[{id:'11',name:'Store',url:'https://store.example',role:'reader'},{id:'12',name:'Store',url:'javascript:alert(1)',role:'reader'}]}));
  assert.equal(result.results.filter(r=>r.kind==='website').length,0);
});
test('short searches use navigation without resource fan-out and validate caller input',async()=>{
  let loaded=0;
  const source=sources({sites:async()=>{loaded++;return[];},navigation:()=>[{id:'nav:mail',kind:'navigation',label:'Mail',href:'/tenants/1/mail'}]});
  const result=await queryConsole({query:'M'},source);assert.equal(loaded,0);assert.equal(result.results[0].label,'Mail');
  for(const input of [{query:'a'.repeat(161)},{query:'Store',customerID:'../2'},{query:'Store',projectID:'0'}])assert.ok((await queryConsole(input,source)).error);
});
test('resource search returns only public navigation metadata and reports limited pages',async()=>{
  const result=await queryConsole({query:'Store',projectID:'2'},sources({apps:async()=>({docs:[{id:'3',name:'Store app',status:'ready',secret:'never-return'}],totalDocs:30})}));
  assert.equal(result.more,true);assert.doesNotMatch(JSON.stringify(result),/never-return|secret/);assert.ok(result.results.some(r=>r.href==='/hosting/apps/3'));
});

test('project scope does not include websites that only have a workspace association',async()=>{
  let siteReads=0;
  const result=await queryConsole({query:'Store',projectID:'2'},sources({sites:async()=>{siteReads++;return tenant.sites;},bridges:async()=>{siteReads++;return{};}}));
  assert.equal(siteReads,0);assert.equal(result.results.some(item=>item.kind==='website'),false);
  assert.ok(result.results.some(item=>item.kind==='application'));assert.ok(result.results.some(item=>item.kind==='database'));
});
