import { useCallback, useEffect, useRef, useState } from "react";
import { z } from "zod";
import { ui, usePluginScope, type CreateProps, type ResourceProps, type PluginUI, type TransferDescriptor } from "@webdock/plugin-ui";

const sourceSchema=z.object({sources:z.array(z.object({id:z.string().regex(/^[1-9][0-9]{0,18}$/),kind:z.enum(["postgres","project","report","buildlog"]),state:z.literal("ready")})).max(301),maxBytes:z.number(),maxRetentionSeconds:z.number(),pgSourceRequiresReadonlyRole:z.boolean()});
const statusSchema=z.object({state:z.enum(["pending","producing","ready","failed","deleting","deleted","expired"]),kind:z.enum(["postgres","project","report","buildlog","diagnostics"]),expires_at:z.number(),source:z.object({version:z.literal(1),kind:z.enum(["postgres","project","report","buildlog"]),id:z.string(),binding:z.string(),commit:z.string().nullable()}).nullable().optional(),failure:z.enum(["readonly_source_unavailable","source_access_changed","output_limit","storage_unavailable","export_failed"]).nullable().optional(),bytes:z.number().nullable(),sha256:z.string().nullable(),jobID:z.string().nullable().optional(),job:z.object({id:z.string(),name:z.string(),state:z.enum(["waiting","active","completed","failed","cancelled"]),attempts:z.number()}).optional()});
type Status=z.infer<typeof statusSchema>;
type Boundary=ReturnType<typeof usePluginScope>;
interface Attempt {action:string;input:Readonly<Record<string,unknown>>;idempotencyKey:string}
function invoke(b:Boundary,context:{plugin:string;projectID:string},action:string,input:Readonly<Record<string,unknown>>,signal:AbortSignal,key:string=crypto.randomUUID()){
 return b.api.request("/api/console/plugins/invoke",z.unknown(),{method:"POST",signal,body:JSON.stringify({pluginID:context.plugin,projectID:context.projectID,action,input,idempotencyKey:key})});
}
function message(error:unknown){return error instanceof Error?error.message:"Export request failed.";}
function ready(status:Status|undefined,clock:number):status is Status{return status?.state==="ready"&&status.job?.state==="completed"&&status.expires_at>clock;}
function stopped(status:Status|undefined){const state=status?.job?.state;return state==="failed"||state==="cancelled";}
const failures={readonly_source_unavailable:"Read-only database access is not reconciled or verified TLS is unavailable.",source_access_changed:"Source access changed. Check permissions and create a new export.",output_limit:"The export exceeds the supported 32 MiB limit.",storage_unavailable:"Storage is unavailable. Inspect the current job before retrying.",export_failed:"The producer could not finish this export."};
const buildlogNotice="Actual stored Core build output, potentially sensitive and may be truncated. Known environment literal values are masked by the build pipeline; complete output and secret-free content are not guaranteed. Maximum 24,000 UTF-8 source bytes and 64 KiB gzip. No raw preview is shown.";
const labels={postgres:"PostgreSQL database",project:"Committed source archive",report:"Observed project report",buildlog:"Stored completed build log",diagnostics:"Submitted diagnostic snapshot"};

export function Create(context:CreateProps){const b=usePluginScope(context);return <CreateForm key={b.key} context={context} b={b}/>;}
function CreateForm({context,b}:{context:CreateProps;b:Boundary}){
 const [kind,setKind]=useState<keyof typeof labels>("report"),[sourceID,setSourceID]=useState(""),[retention,setRetention]=useState(3600);
 const [sources,setSources]=useState<z.infer<typeof sourceSchema>>(),[error,setError]=useState<string>(),[pending,setPending]=useState(false),[retry,setRetry]=useState<Attempt>();
 const [component,setComponent]=useState("storage"),[outcome,setOutcome]=useState("healthy");
 const controller=useRef(new AbortController());
 useEffect(()=>{const ownedController=new AbortController();controller.current=ownedController;const signal=ownedController.signal;void invoke(b,context,"archives.sources",{},signal).then(value=>{if(!b.isCurrent()||signal.aborted)return;const parsed=sourceSchema.parse(value);setSources(parsed);setSourceID(parsed.sources.find(s=>s.kind==="report")?.id??"");}).catch((error:unknown)=>{if(b.isCurrent()&&!signal.aborted)setError(message(error));});return()=>{ownedController.abort();};},[b,context]);
 const choices=sources?.sources.filter(s=>s.kind===kind)??[];
 const chooseKind=(value:keyof typeof labels)=>{setKind(value);setSourceID(sources?.sources.find(s=>s.kind===value)?.id??"");};
 const submit=async()=>{
  if(!b.isCurrent()||pending)return;
  const input=kind==="diagnostics"?{request_id:crypto.randomUUID(),kind,retention_seconds:retention,diagnostics:{version:1,state:"finished",builds:[],checks:[{component,outcome,duration_ms:0}]}}:{request_id:crypto.randomUUID(),kind,source_id:sourceID,retention_seconds:retention};
  const attempt=retry??{action:"archives.create",input:Object.freeze(input),idempotencyKey:crypto.randomUUID()};setRetry(attempt);setPending(true);setError(undefined);
  try{const result=z.object({resource:z.object({id:z.string(),name:z.string()})}).parse(await invoke(b,context,attempt.action,attempt.input,controller.current.signal,attempt.idempotencyKey));if(b.isCurrent()&&!controller.current.signal.aborted)context.onCreated(result.resource.id);}
  catch(error){if(b.isCurrent()&&!controller.current.signal.aborted)setError(message(error));}
  finally{if(b.isCurrent())setPending(false);}
 };
 return <section className={ui.card} aria-label="Create protected export"><form className={ui.cardBody} onSubmit={event=>{event.preventDefault();void submit();}}>
  <h2 className={ui.sectionTitle}>Create protected export</h2>
  <p className={ui.hint}>Create the export, then generate it in the background. Maximum compressed output 32 MiB; access expires within 24 hours.</p>
  <label className={ui.field}>Content<select className={ui.input} value={kind} disabled={pending||Boolean(retry)} onChange={event=> { chooseKind(event.target.value as keyof typeof labels); }}>{Object.entries(labels).map(([value,label])=><option key={value} value={value}>{label}</option>)}</select></label>
  {kind!=="diagnostics"?<label className={ui.field}>Authorized source<select className={ui.input} value={sourceID} disabled={pending||Boolean(retry)||!sources} onChange={event=> { setSourceID(event.target.value); }}>{choices.length?choices.map(source=><option key={source.id} value={source.id}>{labels[source.kind]} {source.id}</option>):<option value="">No current source available</option>}</select></label>:<>
   <p className={ui.hint}>This records a submitted check. It does not attest live service health; choose the observed report for Core measurements.</p>
   <label className={ui.field}>Component<select className={ui.input} value={component} disabled={pending||Boolean(retry)} onChange={e=> { setComponent(e.target.value); }}>{["storage","database","runtime","gateway"].map(v=><option key={v}>{v}</option>)}</select></label>
   <label className={ui.field}>Submitted result<select className={ui.input} value={outcome} disabled={pending||Boolean(retry)} onChange={e=> { setOutcome(e.target.value); }}>{["healthy","unavailable","denied","integrity"].map(v=><option key={v}>{v}</option>)}</select></label>
  </>}
  <label className={ui.field}>Access lifetime<select className={ui.input} value={retention} disabled={pending||Boolean(retry)} onChange={event=> { setRetention(Number(event.target.value)); }}><option value={3600}>1 hour</option><option value={21600}>6 hours</option><option value={86400}>24 hours</option></select></label>
  {kind==="buildlog"?<p className={ui.hint}>{buildlogNotice}</p>:null}
  {kind==="project"?<p className={ui.hint}>Committed files only. Git history, runtime volumes, submodule contents and LFS hydration are not included or guaranteed.</p>:null}
  {kind==="postgres"?<p className={ui.hint}>Dedicated tenant databases only. Read-only access and verified TLS are checked before export. This is a logical export, not point-in-time recovery.</p>:null}
  {error?<p role="alert" className="text-sm text-danger">{error} Retry preserves this exact request.</p>:null}
  <div className="flex flex-wrap gap-2"><button type="submit" className={ui.button("primary")} disabled={pending||kind!=="diagnostics"&&!sourceID}>{pending?"Creating…":retry?"Retry creation":"Create export"}</button><button type="button" className={ui.button()} onClick={()=>{controller.current.abort();context.onCancel();}}>Cancel</button></div>
 </form></section>;
}

export function Resource(context:ResourceProps){const b=usePluginScope(context,JSON.stringify([context.writable,context.features,context.actions]));return <ResourceBody key={b.key} context={context} b={b}/>;}
function ResourceBody({context,b}:{context:ResourceProps;b:Boundary}){
 const [status,setStatus]=useState<Status>(),[error,setError]=useState<string>(),[busy,setBusy]=useState<string>(),[retry,setRetry]=useState<Attempt>(),[link,setLink]=useState<{url:string;filename:string}>(),[confirm,setConfirm]=useState(false);
 const lifetime=useRef(new AbortController()),active=useRef<AbortController|null>(null),url=useRef<string|undefined>(undefined),poll=useRef<ReturnType<typeof setTimeout>|undefined>(undefined);
 const [clock,setClock]=useState(()=>Date.now()/1000);
 const hide=useCallback(()=>{active.current?.abort();active.current=null;if(url.current){URL.revokeObjectURL(url.current);url.current=undefined;}setLink(undefined);setBusy(undefined);},[]);
 const refresh=useCallback(async function pollStatus(){
  if(!b.isCurrent())return;
  try{const view=statusSchema.parse(await invoke(b,context,"archives.status",Object.freeze({resource_id:context.resource.id}),lifetime.current.signal));if(!b.isCurrent())return;setStatus(view);setError(undefined);
   if(view.job&&["waiting","active"].includes(view.job.state))poll.current=setTimeout(()=>{void pollStatus();},2000);
  }catch(error){if(b.isCurrent()&&!lifetime.current.signal.aborted){setError(message(error));hide();}}
 },[b,context,hide]);
 useEffect(()=>{const owner=new AbortController();lifetime.current=owner;void refresh();return()=>{owner.abort();if(poll.current)clearTimeout(poll.current);hide();};},[refresh,hide]);
 useEffect(()=>{const timer=setInterval(()=>{setClock(Date.now()/1000);if(!b.isCurrent())hide();},1000);return()=>{clearInterval(timer);};},[b,hide]);
 const allowed=(action:string,write=false)=>(!write||context.writable&&context.features.includes("exports"))&&(!context.actions||context.actions.includes(action));
 const change=async(action:"archives.generate"|"archives.delete")=>{
  if(!b.isCurrent()||!allowed(action,true)||busy)return;
  hide();const controller=new AbortController();active.current=controller;
  const attempt=(retry?.action===action?retry:undefined)??{action,input:Object.freeze({resource_id:context.resource.id}),idempotencyKey:crypto.randomUUID()};setRetry(attempt);setBusy(attempt.action);setError(undefined);
  try{await invoke(b,context,attempt.action,attempt.input,controller.signal,attempt.idempotencyKey);if(!b.isCurrent()||active.current!==controller)return;setRetry(undefined);if(attempt.action==="archives.delete")context.onDeleted();else await refresh();}
  catch(error){if(b.isCurrent()&&!controller.signal.aborted)setError(message(error));}
  finally{if(b.isCurrent()&&active.current===controller){active.current=null;setBusy(undefined);}}
 };
 const download=async()=>{
  if(!b.isCurrent()||!allowed("archives.download")||!ready(status,Date.now()/1000)||busy)return;
  hide();const controller=new AbortController();active.current=controller;setBusy("download");setError(undefined);
  const isCurrent=()=>b.isCurrent()&&active.current===controller;
  try{
   const raw=await invoke(b,context,"archives.download",Object.freeze({resource_id:context.resource.id}),controller.signal);
   if(!isCurrent())return;
   const descriptor=raw as TransferDescriptor;
   if(descriptor.method!=="GET"||descriptor.exactBytes!==status.bytes||descriptor.expectedSha256!==status.sha256||descriptor.exactBytes>(status.kind==="buildlog"?65536:33554432)||descriptor.expiresAt>status.expires_at)throw Error("Download differs from the completed export receipt.");
   const blob=await b.api.downloadBinary(descriptor,{signal:controller.signal,isCurrent});
   if(!isCurrent())return;url.current=URL.createObjectURL(blob);setLink({url:url.current,filename:descriptor.attachment.filename});
  }catch(error){if(isCurrent()&&!controller.signal.aborted)setError(message(error));}
  finally{if(isCurrent()){active.current=null;setBusy(undefined);}}
 };
 const failed=stopped(status);
 return <section className={ui.card} aria-label="Protected export"><div className={ui.cardBody}>
  <h2 className={ui.sectionTitle}>{status?labels[status.kind]:"Protected export"}</h2>
  {status?.kind==="buildlog"?<p className={ui.hint}>{buildlogNotice}</p>:null}
  {!status&&!error?<p role="status">Loading export…</p>:null}
  {status?<><dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm"><dt>Output</dt><dd>{failed&&status.state!=="ready"?"Stopped":status.state}</dd><dt>Access expires</dt><dd>{new Date(status.expires_at*1000).toLocaleString()}</dd><dt>Compressed size</dt><dd>{status.bytes===null?"Not finished":`${status.bytes.toLocaleString()} bytes`}</dd>{status.sha256?<><dt>SHA-256</dt><dd className={`${ui.mono} break-all`}>{status.sha256}</dd></>:null}</dl>
   {status.source?.commit?<p className={`${ui.mono} break-all`}>Committed snapshot: {status.source.commit}</p>:null}
   {status.job?<p role="status" className="text-sm">Job: {status.job.state} · attempt {status.job.attempts}</p>:null}
   {status.failure?<p role="alert" className="text-sm text-danger">{failures[status.failure]}</p>:null}
   {failed?<p role="alert" className="text-sm text-danger">The export job stopped. No successful output is promised. Check source access and limits; delete this request before creating another export.</p>:null}
  </>:null}
  <p className={ui.hint}>Maximum 32 MiB. Expiry blocks downloads; retained bytes follow the configured storage policy. Delete explicitly requests removal of this export.</p>
  {error?<p role="alert" className="text-sm text-danger">{error}</p>:null}
  <div className="flex flex-wrap gap-2">
   <button type="button" className={ui.button("primary")} disabled={!allowed("archives.generate",true)||Boolean(busy)||Boolean(status?.jobID)&&!retry||retry?.action==="archives.delete"} onClick={()=>void change("archives.generate")}>{busy==="archives.generate"?"Queuing…":retry?.action==="archives.generate"?"Retry generation":"Generate export"}</button>
   <button type="button" className={ui.button()} disabled={!allowed("archives.download")||Boolean(busy)||!ready(status,clock)} onClick={()=>void download()}>{busy==="download"?"Downloading and verifying…":"Download"}</button>
   <button type="button" className={ui.button("quiet")} onClick={()=>{if(poll.current)clearTimeout(poll.current);void refresh();}}>Refresh status</button>
   {busy==="download"||link?<button type="button" className={ui.button("quiet")} onClick={()=>{hide();setBusy(undefined);}}>Hide download</button>:null}
  </div>
  {link?<a className={ui.button()} href={link.url} download={link.filename}>Save {link.filename}</a>:null}
  {allowed("archives.delete",true)?<div className="grid gap-2 border-t border-outline pt-4"><label className="flex gap-2 text-sm"><input type="checkbox" checked={confirm} disabled={Boolean(busy)||retry?.action==="archives.delete"} onChange={e=> { setConfirm(e.target.checked); }}/>Delete this export and stop any pending job</label><button type="button" className={ui.button("danger")} disabled={!confirm||Boolean(busy)} onClick={()=>void change("archives.delete")}>{busy==="archives.delete"?"Deleting…":retry?.action==="archives.delete"?"Retry deletion":"Delete export"}</button></div>:null}
 </div></section>;
}
export default {Resource,Create:{"archives.create":Create}} satisfies PluginUI;
