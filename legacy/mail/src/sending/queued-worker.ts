import {fileQueuedWebmail,type WebmailClientFactory} from '../api/webmail-sending.ts';
import {createHash} from 'node:crypto';
import type {SendLease} from '@webdock/mail-core/outbox';
import type {MailControl} from '../api/server.ts';
import type {PrivatePayloadStore} from './payload-store.ts';
import {parseStoredRequest,compileSendRequest} from './request.ts';
import {dispatchOne,type DispatchAuthorization} from './dispatch.ts';
export async function processQueuedSend(control:MailControl,store:PrivatePayloadStore,transports:Parameters<typeof dispatchOne>[2]={},webmailFactory?:WebmailClientFactory){
 return dispatchOne({
  claim:async()=>await control({action:'send.claim'}) as SendLease|null,
  prepareFailed:async lease=>await control({action:'send.reviewPreparation',lease})===true,
  begin:async(lease,digest)=>await control({action:'send.begin',lease,digest}) as DispatchAuthorization|null,
  finish:async(lease,result)=>{
   const recorded=await control({action:'send.finish',lease,result});
   if(recorded===true && result.state==='accepted' && lease.payloadFormat==='mime'){
    try{await fileQueuedWebmail(control,store,{id:lease.id,customerID:lease.customerID,leaseToken:lease.leaseToken},webmailFactory);}
    catch{await control({action:'send.webmail.filed',id:lease.id,customerID:lease.customerID,leaseToken:lease.leaseToken,filed:false}).catch(()=>{});}
   }else if(recorded===true && result.state!=='unknown')await store.remove(lease.payloadRef);
   return recorded===true;
  },
 },async job=>{
  const payload=await store.get(job.customerID,job.payloadRef);
  if(payload.length!==job.payloadBytes || createHash('sha256').update(payload).digest('hex')!==job.requestHash)throw Error('Mail payload integrity check failed.');
  if(job.payloadFormat==='mime'){
   if(job.route.kind!=='stalwart' || typeof job.envelope.from!=='string' || !Array.isArray(job.envelope.to) || !job.envelope.to.every(value=>typeof value==='string') || typeof job.sourceContext.senderName!=='string')throw Error('Invalid queued Webmail envelope.');
   return {sender:{email:job.envelope.from,name:job.sourceContext.senderName},to:job.envelope.to as string[],subject:'',raw:payload};
  }
  const request=parseStoredRequest(JSON.parse(payload.toString('utf8')));
  const envelope={from:request.from.email,to:[...request.to,...request.cc??[],...request.bcc??[]]};
  if(envelope.from!==job.envelope.from || JSON.stringify(envelope.to)!==JSON.stringify(job.envelope.to))throw Error('Mail envelope does not match its admission.');
  return compileSendRequest(request);
 },transports);
}

export async function cleanupQueuedPayloads(control:MailControl,store:PrivatePayloadStore){
 const rows=await control({action:'send.cleanup'}) as {id:string;customerID:string;ref:string}[];
 if(!Array.isArray(rows) || rows.length>50)throw Error('Invalid Mail cleanup response.');
 for(const row of rows){if(!/^[1-9][0-9]{0,18}$/.test(row.id) || !/^[1-9][0-9]{0,18}$/.test(row.customerID))throw Error('Invalid Mail cleanup scope.');await store.remove(row.ref);}
 if(rows.length)await control({action:'send.cleanupDone',items:rows});
 return rows.length;
}
