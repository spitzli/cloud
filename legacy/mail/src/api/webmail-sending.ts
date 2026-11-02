import {randomUUID} from 'node:crypto';
import {z} from 'zod';
import {JmapClient} from '../stalwart/jmap.ts';
import {prepareMessage} from '../sending/message.ts';
import type {PrivatePayloadStore} from '../sending/payload-store.ts';
import type {MailControl} from './server.ts';
const resource=z.string().regex(/^[1-9][0-9]{0,18}$/),provider=z.string().regex(/^[A-Za-z0-9_-]{1,255}$/);
const accessSchema=z.object({customerID:resource,mailboxID:resource,accountID:provider,runtimeOrigin:z.string(),authorization:z.string().max(4096),senderAddress:z.string().max(254),senderName:z.string().max(80)});
export type WebmailAccess=z.infer<typeof accessSchema>;
export type WebmailClientFactory=(access:WebmailAccess)=>Pick<JmapClient,'exportDraft'|'fileSent'>;
export function webmailClient(value:unknown,factory?:WebmailClientFactory){
 const access=accessSchema.parse(value);
 if(access.runtimeOrigin!==`http://app-${access.customerID}.wd-mail-${access.customerID}.svc.cluster.local:8080`)throw Error('Invalid queued Mail runtime.');
 if(factory){if(process.env.NODE_ENV==='production')throw Error('Test Mail client is unavailable in production.');return factory(access);}
 return new JmapClient({url:access.runtimeOrigin,allowInsecureHttp:true,authorization:access.authorization,accountId:access.accountID,sender:{email:access.senderAddress,name:access.senderName}});
}
const request=z.object({token:z.string().regex(/^[A-Za-z0-9_-]{43}$/),requestID:z.string().uuid(),emailID:provider,identityID:provider}).strict();
export async function enqueueWebmail(control:MailControl,store:PrivatePayloadStore,input:unknown,factory?:WebmailClientFactory){
 const command=request.parse(input);
 const prepared=await control({action:'send.webmail.prepare',submission:command}) as {existing:boolean;id?:string;state?:string;filed?:boolean;filingState?:string;operationID?:string;leaseToken?:string;access?:unknown};
 if(prepared.existing===true)return {id:resource.parse(prepared.id),state:prepared.state,filed:prepared.filed===true,filingState:prepared.filingState};
 const operationID=resource.parse(prepared.operationID),leaseToken=z.string().uuid().parse(prepared.leaseToken);
 try{
  const access=accessSchema.parse(prepared.access),client=webmailClient(access,factory);
  const draft=await client.exportDraft(command.emailID,command.identityID);
  if(draft.sender.email!==access.senderAddress)throw Error('Draft sender changed.');
  const message=prepareMessage(draft.raw,{email:access.senderAddress,name:access.senderName},draft.to),ref=randomUUID();
  const staged=await control({action:'send.webmail.stage',submission:{...command,operationID,leaseToken,payloadRef:ref,payloadBytes:message.raw.length,requestHash:message.hash,envelope:{from:message.from,to:message.to}}}) as {id:string;created:boolean;state:string;customerID:string;payloadRef:string;uploadToken:string};
  if(staged.id!==operationID || staged.created!==true || staged.state!=='staging' || staged.customerID!==access.customerID || staged.payloadRef!==ref)throw Error('Mail staging receipt is invalid.');
  const upload={id:staged.id,customerID:staged.customerID,payloadRef:ref,uploadToken:z.string().uuid().parse(staged.uploadToken)};
  try{await store.put(access.customerID,message.raw,ref);}catch(error){await control({action:'send.abortPayload',lease:upload}).catch(()=>{});throw error;}
  const committed=await control({action:'send.commitPayload',lease:upload}) as {queued?:boolean};if(committed?.queued!==true)throw Error('Mail queue acknowledgement is unavailable.');
  return {id:staged.id,state:'queued',filed:false};
 }catch(error){await control({action:'send.webmail.abort',operationID,leaseToken}).catch(()=>{});throw error;}
}
export async function fileQueuedWebmail(control:MailControl,store:PrivatePayloadStore,input:{id:string;token?:string;customerID?:string;leaseToken?:string},factory?:WebmailClientFactory){
 const info=await control({action:'send.webmail.filingAccess',...input}) as {access:unknown;emailID:string;complete:boolean;lease:{id:string;customerID:string;leaseToken:string};payloadRef:string};
 if(info.complete)return {id:input.id,state:'accepted',filed:true};
 let filed=false;
 try{await webmailClient(info.access,factory).fileSent(provider.parse(info.emailID));filed=true;}catch{ /* Receipt is already durable; filing never causes another send. */ }
 const result=await control({action:'send.webmail.filed',...info.lease,filed}) as {recorded?:boolean;filed?:boolean};
 if(result.recorded!==true)throw Error('Sent filing could not be recorded.');
 if(result.filed)await store.remove(info.payloadRef);
 return {id:input.id,state:'accepted',filed:result.filed===true};
}
