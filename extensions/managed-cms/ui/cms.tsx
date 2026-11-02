import React, { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { CMSApp } from "@webdock/cms-ui/client";
import { createBridgeTransport } from "@webdock/cms-bridge/client";
import type { ManagedAppProvisioning } from "@webdock/plugin-sdk/contracts";
import { discoverCMS, type CMSSite } from "./discovery";
import "@webdock/cms-ui/styles.css";
import "./cms.css";

type Call=(name:string,input?:unknown)=>Promise<any>;
const stepLabels:Record<string,string>={application:"Prepare the CMS",database:"Create the database",bindings:"Connect the database",source:"Prepare the application",build:"Build the application",release:"Initialize and start the CMS",readiness:"Verify the secure website address"};
const stateLabels:Record<string,string>={pending:"Waiting",running:"In progress",succeeded:"Done",skipped:"Not needed",blocked:"Needs attention",failed:"Failed"};
function openChannel(onReady:(call:Call)=>void,onError:(error:string)=>void){
 const nonce=location.hash.slice(1),pending=new Map<string,{resolve:(value:any)=>void;reject:(error:Error)=>void;timeout:ReturnType<typeof setTimeout>}>();let port:MessagePort|undefined;
 const timeout=setTimeout(()=>onError("Open CMS from your Spitzli Cloud project to connect."),10000);
 const receive=(event:MessageEvent)=>{
  if(port||event.source!==parent||event.data?.type!=="webdock:init"||!nonce||event.data.nonce!==nonce||event.ports.length!==1)return;
  clearTimeout(timeout);port=event.ports[0];port.onmessage=(event)=>{
   const request=pending.get(event.data?.id);if(!request)return;
   clearTimeout(request.timeout);pending.delete(event.data.id);
   if(event.data.error)request.reject(Error(String(event.data.error)));else request.resolve(event.data.result);
  };
  onReady((name,input={})=>new Promise((resolve,reject)=>{
   const id=crypto.randomUUID(),message={type:"action",id,name,input};
   if(new TextEncoder().encode(JSON.stringify(message)).length>32768)return reject(Error("CMS request exceeds the transfer limit."));
   const timeout=setTimeout(()=>{pending.delete(id);reject(Error("Request timed out. Refresh status before trying again."));},30000);
   pending.set(id,{resolve,reject,timeout});port!.postMessage(message);
  }));
 };
 window.addEventListener("message",receive);
 return()=>{clearTimeout(timeout);window.removeEventListener("message",receive);port?.close();for(const request of pending.values()){clearTimeout(request.timeout);request.reject(Error("CMS disconnected"));}pending.clear();};
}
// Install the listener before iframe load; React effects may run after the host transfers its port.
const connection=new Promise<Call>((resolve,reject)=>{
 const close=openChannel(resolve,error=>reject(Error(error)));
 window.addEventListener("pagehide",close,{once:true});
});
function App(){
 const [call,setCall]=useState<Call|null>(null),[site,setSite]=useState<CMSSite|null>(null),[instance,setInstance]=useState<ManagedAppProvisioning|null>(null),[loaded,setLoaded]=useState(false),[error,setError]=useState(""),[busy,setBusy]=useState(false),[name,setName]=useState("My website"),[requestID]=useState(()=>crypto.randomUUID());
 const transport=useMemo(()=>call?createBridgeTransport(request=>call(request.method==="GET"?"cms.read":"cms.write",request.method==="GET"?{path:request.path}:request.body)):undefined,[call]);
 useEffect(()=>{let active=true;void connection.then(value=>{if(active)setCall(()=>value);},error=>{if(active)setError(error.message);});return()=>{active=false;};},[]);
 const refresh=async()=>{
  if(!call||!transport)return;setBusy(true);setError("");
  try{const result=await discoverCMS(transport,()=>call("cms.instances"));setSite(result.site);setInstance(result.instance);setLoaded(true);if(result.instance?.state==="ready"&&!result.site)setError("Your CMS is ready, but the editor connection is unavailable. Check website access and refresh.");}catch(e){setError(e instanceof Error?e.message:"CMS is unavailable.");}finally{setBusy(false);}
 };
 useEffect(()=>{if(call)void refresh();},[call]);
 useEffect(()=>{
  if(!call||!instance||!["queued","running"].includes(instance.state))return;
  let active=true;const timer=setTimeout(()=>{void call("cms.status",{provisioning_id:instance.provisioning_id}).then(value=>{if(active)setInstance(value);},e=>{if(active)setError(e.message);});},5000);
  return()=>{active=false;clearTimeout(timer);};
 },[call,instance]);
 useEffect(()=>{if(instance?.state==="ready"&&!site)void refresh();},[instance?.state]);
 const create=async(event:React.SyntheticEvent)=>{
  event.preventDefault();if(!call||busy||!name.trim())return;setBusy(true);setError("");
  try{setInstance(await call("cms.create",{name,request_id:requestID}));}catch(e){setError(e instanceof Error?e.message:"CMS creation failed.");}finally{setBusy(false);}
 };
 if(site&&transport)return <CMSApp embedded siteName={site.name} siteURL={site.origin} accountURL={site.origin} accessURL={site.origin} transport={transport} uiLocale={navigator.language.startsWith("de")?"de":"en"}/>;
 return <main className="managed-cms-setup">
  <p className="eyebrow">PROJECT CONTENT</p><h1>Your website’s CMS</h1>
  <p className="intro">Create pages, manage media and publish from Spitzli Cloud. Each project gets its own CMS, database and media storage.</p>
  {error&&<p className="error" role="alert">{error}</p>}
  {!loaded&&!error&&<p role="status">Connecting to your project…</p>}
  {loaded&&!instance&&<form onSubmit={create}>
   <label htmlFor="site-name">Website name</label><input id="site-name" required maxLength={160} value={name} onChange={event=>setName(event.target.value)} onKeyDown={event=>{if(event.key==="Enter")void create(event);}} disabled={busy}/>
   <p>No accessible CMS connection was found. Create one for a new website, or ask your operator to connect an existing website.</p><p>Uses the CMS template and resources approved for this project.</p><button type="button" onClick={event=>void create(event)} disabled={!call||busy||!name.trim()}>{busy?"Creating…":"Create CMS"}</button>
  </form>}
  {instance&&<section aria-live="polite"><h2>{instance.state==="ready"?"Opening editor…":instance.state==="blocked"||instance.state==="failed"?"Setup needs attention":"Setting up your CMS"}</h2>
   <ol className="steps">{instance.steps.filter(step=>step.state!=="skipped").map(step=><li key={step.name}><span>{stepLabels[step.name]??step.name.replaceAll("-"," ")}</span><strong>{stateLabels[step.state]??step.state}</strong></li>)}</ol>
   {instance.reason&&<p>{instance.reason}</p>}
   {["blocked","failed"].includes(instance.state)&&<button type="button" disabled={busy} onClick={async()=>{
    if(!call)return;setBusy(true);setError("");
    try{setInstance(await call("cms.create",{name:"CMS",request_id:crypto.randomUUID()}));}catch(e){setError(e instanceof Error?e.message:"Could not resume setup.");}finally{setBusy(false);}
   }}>Resume setup</button>}
   <p>Progress is saved. You can leave this page while setup continues.</p>
  </section>}
  {call&&<button className="secondary" type="button" onClick={()=>void refresh()} disabled={busy}>Refresh status</button>}
  <p className="retention">Disabling the plugin keeps published content, the database and media. Suspend the hosted instance to take the website offline.</p>
 </main>;
}
createRoot(document.getElementById("root")!).render(<App/>);
