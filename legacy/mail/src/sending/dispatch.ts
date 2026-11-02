import {messageDigest} from '@webdock/mail-core/submission-proof';
import type {SendLease} from '@webdock/mail-core/outbox';
import {submitRelayAPI,RelayAPIFailure} from './relay-api.ts';
import {submitSMTP,SMTPFailure} from './smtp.ts';
import {prepareMessage,stampMessage} from './message.ts';
export type DispatchMessage=Omit<Parameters<typeof submitRelayAPI>[1],'referenceID'>;
export type DispatchAuthorization=
 | {kind:'direct';account:Parameters<typeof submitRelayAPI>[0]}
 | {kind:'stalwart';smtp:Parameters<typeof submitSMTP>[0];proof:string};
type Receipt={state:'accepted'|'rejected'|'unknown';providerID?:string};
export type DispatchControl={
 claim:()=>Promise<SendLease|null>;
 /** Reauthorize source, sender, route revision and budget; commit handoff before returning credentials. */
 begin:(job:SendLease,digest:string)=>Promise<DispatchAuthorization|null>;
 finish:(job:SendLease,result:Receipt)=>Promise<boolean>;
 prepareFailed?:(job:SendLease)=>Promise<boolean>;
};
/** No retry loop: expired preparation leases can be reclaimed, but handoffs cannot. */
export async function dispatchOne(control:DispatchControl,load:(job:SendLease)=>Promise<DispatchMessage>,transports:{relay?:typeof submitRelayAPI;smtp?:typeof submitSMTP}={}){
 const job=await control.claim();if(!job)return false;
 let message:DispatchMessage,prepared:ReturnType<typeof prepareMessage>;
 try{message=await load(job);prepared=prepareMessage(message.raw,message.sender,[...message.to,...message.cc??[],...message.bcc??[]]);}
 catch(error){await control.prepareFailed?.(job);throw error;}
 const authorization=await control.begin(job,messageDigest(prepared.raw));if(!authorization)return true;
 let result:Receipt;
 try{
  if(authorization.kind!==job.route.kind)throw Error('Mail route changed.');
  if(authorization.kind==='direct'){
   const accepted=await (transports.relay??submitRelayAPI)(authorization.account,{...message,referenceID:job.id});
   result={state:'accepted',providerID:accepted.providerID};
  }else{
   await (transports.smtp??submitSMTP)(authorization.smtp,{from:prepared.from,to:prepared.to,raw:stampMessage(prepared.raw,job.id,authorization.proof)});
   // SMTP receipt identifies Webdock's submission, not final external delivery.
   result={state:'accepted',providerID:job.id};
  }
 }catch(error){result={state:error instanceof RelayAPIFailure || error instanceof SMTPFailure ? error.uncertain?'unknown':'rejected':'unknown'};}
 // A lost completion is never handled by retrying the transport.
 if(!await control.finish(job,result))throw Error('Mail receipt could not be recorded.');
 return true;
}
