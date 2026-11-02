import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { Window } from "happy-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ApiClient, type Session } from "../../../apps/console/src/api";
import { ApiContext, SessionContext } from "../../../apps/console/src/context";
import {Create,Resource} from "../ui/index";
const session:Session={user:{id:"unit",name:"Unit",email:"unit@example.test",role:"operator",emailVerified:true},session:{id:"session-one",userId:"unit",expiresAt:"2099-01-01T00:00:00Z"}};
const context={plugin:"archives",projectID:"124",customer:"123",resource:{id:"11111111-1111-4111-8111-111111111111",offering:"archive",name:"Report",createdVia:"self-service" as const},writable:true,features:["exports"],actions:["archives.generate","archives.status","archives.download","archives.delete"],onDeleted(){}};
async function flush(){await act(async()=>{await new Promise(r=>setTimeout(r,10));});}
await test("create retry keeps the complete source/input/idempotency identity frozen",async()=>{
 const browser=new Window({url:"https://console.example"});Object.assign(globalThis,{window:browser,document:browser.document,HTMLElement:browser.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
 const requests:unknown[]=[];let created="";
 const api=new ApiClient({apiOrigin:"https://console.example",authOrigin:"https://account.example",enabledFeatures:[]},async(_url,init)=>{
  const body=JSON.parse(String(init?.body));if(body.action==="archives.sources")return Response.json({sources:[{id:"124",kind:"report",state:"ready"}],maxBytes:33554432,maxRetentionSeconds:86400,pgSourceRequiresReadonlyRole:true});
  requests.push(body);if(requests.length===1)throw Error("lost response");return Response.json({resource:{id:context.resource.id,name:"Report"}});
 });
 const host=document.createElement("div"),root=createRoot(host);document.body.append(host);const client=new QueryClient();
 try{
  await act(async()=>{root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><Create {...{plugin:"archives",projectID:"124",customer:"123",action:"archives.create",title:"Export",resources:[],onCreated:(id:string)=>{created=id;},onCancel(){}}}/></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>);});await flush();
  const click=async()=>{const button=[...host.querySelectorAll("button")].find(b=>b.textContent?.includes("Create export")||b.textContent?.includes("Retry creation"));assert(button);await act(async()=>{button.click();});await flush();};
  await click();await click();assert.equal(requests.length,2);assert.deepEqual(requests[0],requests[1]);assert.equal(created,context.resource.id);
 }finally{await act(async()=>root.unmount());client.clear();await browser.happyDOM.abort();}
});
await test("download descriptor and bytes stay outside caches and retire with the resource scope",async()=>{
 const browser=new Window({url:"https://console.example"});Object.assign(globalThis,{window:browser,document:browser.document,HTMLElement:browser.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
 const revoked:string[]=[];const oldCreate=URL.createObjectURL,oldRevoke=URL.revokeObjectURL;URL.createObjectURL=()=>"blob:archive-owned";URL.revokeObjectURL=value=>{revoked.push(value);};
 const api=new ApiClient({apiOrigin:"https://console.example",authOrigin:"https://account.example",enabledFeatures:[]},async(_url,init)=>{
  const body=JSON.parse(String(init?.body));if(body.action==="archives.status")return Response.json({state:"ready",kind:"report",expires_at:Math.floor(Date.now()/1000)+3600,bytes:3,sha256:"a".repeat(64),jobID:"job-"+"b".repeat(64),job:{id:"job-"+"b".repeat(64),name:"archives.produce",state:"completed",attempts:1}});
  return Response.json({version:1,id:"22222222-2222-4222-8222-222222222222",path:"/api/console/plugins/storage/22222222-2222-4222-8222-222222222222",method:"GET",expiresAt:Math.floor(Date.now()/1000)+240,exactBytes:3,expectedSha256:"a".repeat(64),attachment:{filename:"project-report.csv.gz",contentType:"application/gzip"}});
 });
 api.downloadBinary=async()=>new Blob(["abc"],{type:"application/gzip"});
 const client=new QueryClient(),host=document.createElement("div"),root=createRoot(host);document.body.append(host);
 const render=async(projectID:string)=>{await act(async()=>root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><Resource {...context} projectID={projectID}/></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>));await flush();};
 try{
  await render("124");const button=[...host.querySelectorAll("button")].find(b=>b.textContent==="Download");assert(button);await act(async()=>button.click());await flush();assert(host.querySelector('a[href="blob:archive-owned"]'));
  assert(!JSON.stringify(client.getQueryCache().getAll()).includes("22222222"));assert.equal(client.getMutationCache().getAll().length,0);
  await render("125");assert.equal(host.querySelector('a[href="blob:archive-owned"]'),null);assert(revoked.includes("blob:archive-owned"));
 }finally{await act(async()=>root.unmount());client.clear();URL.createObjectURL=oldCreate;URL.revokeObjectURL=oldRevoke;await browser.happyDOM.abort();}
});
await test("delete supersedes a failed generation with a separate frozen retry identity",async()=>{
 const browser=new Window({url:"https://console.example"});Object.assign(globalThis,{window:browser,document:browser.document,HTMLElement:browser.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
 const requests:{action:string;input:unknown;idempotencyKey:string}[]=[];let deleted=false;
 const api=new ApiClient({apiOrigin:"https://console.example",authOrigin:"https://account.example",enabledFeatures:[]},async(_url,init)=>{
  const body=JSON.parse(String(init?.body));if(body.action==="archives.status")return Response.json({state:"pending",kind:"postgres",expires_at:Math.floor(Date.now()/1000)+3600,bytes:null,sha256:null,jobID:null});
  requests.push(body);if(body.action==="archives.generate")throw Error("Source binding changed after rotation");
  if(requests.filter(r=>r.action==="archives.delete").length===1)throw Error("delete reply lost");return Response.json({});
 });
 const client=new QueryClient(),host=document.createElement("div"),root=createRoot(host);document.body.append(host);
 const click=async(label:string)=>{const button=[...host.querySelectorAll("button")].find(b=>b.textContent===label);assert(button);assert.equal(button.disabled,false,label+" must be enabled");await act(async()=>button.click());await flush();};
 try{
  await act(async()=>root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><Resource {...context} onDeleted={()=>{deleted=true;}}/></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>));await flush();
  await click("Generate export");await click("Retry generation");assert.deepEqual(requests[0],requests[1]);await click("Refresh status");
  const confirm=host.querySelector<HTMLInputElement>('input[type="checkbox"]');assert(confirm);assert.equal(confirm.disabled,false,"failed Generate must leave delete confirmation usable");await act(async()=>confirm.click());await flush();
  await click("Delete export");assert(!deleted);await click("Retry deletion");assert(deleted);assert.equal(requests.length,4);
  assert.equal(requests[2].action,"archives.delete");assert.deepEqual(requests[2],requests[3]);assert.notEqual(requests[2].idempotencyKey,requests[0].idempotencyKey);assert.deepEqual(requests[2].input,{resource_id:context.resource.id});
 }finally{await act(async()=>root.unmount());client.clear();await browser.happyDOM.abort();}
});
await test("buildlog creation selects only a Core build ID and warns about stored sensitive excerpts",async()=>{
 const browser=new Window({url:"https://console.example"});Object.assign(globalThis,{window:browser,document:browser.document,HTMLElement:browser.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
 let submitted:Record<string,unknown>|undefined;
 const api=new ApiClient({apiOrigin:"https://console.example",authOrigin:"https://account.example",enabledFeatures:[]},async(_url,init)=>{
  const body=JSON.parse(String(init?.body));if(body.action==="archives.sources")return Response.json({sources:[{id:"126",kind:"buildlog",state:"ready"}],maxBytes:33554432,maxRetentionSeconds:86400,pgSourceRequiresReadonlyRole:true});
  submitted=body.input;return Response.json({resource:{id:context.resource.id,name:"Stored completed build log"}});
 });
 const host=document.createElement("div"),root=createRoot(host);document.body.append(host);const client=new QueryClient();
 try{
  await act(async()=>root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><Create {...{plugin:"archives",projectID:"124",customer:"123",action:"archives.create",title:"Export",resources:[],onCreated(){},onCancel(){}}}/></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>));await flush();
  const content=host.querySelector<HTMLSelectElement>('select');assert(content);assert([...content.options].some(o=>o.value==="buildlog"),"actual completed build logs must be selectable");
  await act(async()=>{content.value="buildlog";content.dispatchEvent(new browser.Event("change",{bubbles:true}) as unknown as Event);});await flush();
  assert(host.textContent?.includes("potentially sensitive"));assert(host.textContent?.includes("may be truncated"));assert.equal(host.querySelector("textarea,pre,iframe"),null);
  const button=[...host.querySelectorAll("button")].find(b=>b.textContent==="Create export");assert(button);assert.equal(button.disabled,false);await act(async()=>button.click());await flush();
  assert.equal(submitted?.kind,"buildlog");assert.equal(submitted?.source_id,"126");assert.deepEqual(Object.keys(submitted??{}).sort(),["kind","request_id","retention_seconds","source_id"]);
 }finally{await act(async()=>root.unmount());client.clear();await browser.happyDOM.abort();}
});
