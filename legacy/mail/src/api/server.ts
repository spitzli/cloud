import {enqueueWebmail,fileQueuedWebmail,type WebmailClientFactory} from './webmail-sending.ts';
import {submissionPolicy} from './submission-policy.ts';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {createHash,randomUUID,timingSafeEqual} from 'node:crypto';
import {parseSendRequest,compileSendRequest} from '../sending/request.ts';
import type {PrivatePayloadStore} from '../sending/payload-store.ts';
export type MailControl=(body:Record<string,unknown>)=>Promise<unknown>;
export class MailControlError extends Error{readonly status:number;constructor(status:number){super('Mail access is unavailable.');this.status=status;}}
function reply(response:ServerResponse,status:number,body:unknown){response.writeHead(status,{'Content-Type':'application/json','Cache-Control':'no-store','X-Content-Type-Options':'nosniff',...(status>=400?{Connection:'close'}:{})});response.end(JSON.stringify(body));}
async function readJSON(request:IncomingMessage,limit=16*1024*1024){
 if(!/^application\/json(?:;|$)/i.test(request.headers['content-type']??''))throw new MailControlError(415);
 const chunks:Buffer[]=[];let size=0;
 const timer=setTimeout(()=>request.destroy(),20000);
 try{for await(const chunk of request){size+=chunk.length;if(size>limit)throw new MailControlError(413);chunks.push(Buffer.from(chunk));}return JSON.parse(Buffer.concat(chunks).toString('utf8'));}
 finally{clearTimeout(timer);}
}
export function createMailAPI(control:MailControl,store:PrivatePayloadStore,options:{webmailSecret?:string;clientFactory?:WebmailClientFactory}={}){
 let active=0;const customers=new Set<string>();
 const server=createServer(async(request,response)=>{
  if((request.url==='/healthz' || request.url==='/health') && request.method==='GET'){reply(response,200,{ok:true});return;}
  if(active>=4){reply(response,429,{error:'Mail is busy. Try later.'});return;}active++;
  let customer:string|undefined,submissionID:string|undefined;
  try{
   if(request.url==='/internal/webmail' && request.method==='POST'){
    const supplied=request.headers.authorization?.match(/^Bearer (\S+)$/)?.[1],expected=options.webmailSecret;
    if(!expected || expected.length<32 || !supplied || supplied.length>512 || !timingSafeEqual(createHash('sha256').update(supplied).digest(),createHash('sha256').update(expected).digest()))throw new MailControlError(401);
    const body=await readJSON(request,2048) as Record<string,unknown>;
    if(!body || typeof body!=='object' || typeof body.action!=='string')throw new MailControlError(400);
    const {action,...input}=body;
    if(action==='submit'){const result=await enqueueWebmail(control,store,input,options.clientFactory);reply(response,200,result);return;}
    if(action==='lookup'){if(Object.keys(input).some(key=>!['emailID','token'].includes(key)) || typeof input.emailID!=='string' || !/^[A-Za-z0-9_-]{1,255}$/.test(input.emailID) || typeof input.token!=='string' || !/^[A-Za-z0-9_-]{43}$/.test(input.token))throw new MailControlError(400);reply(response,200,await control({action:'send.webmail.lookup',token:input.token,emailID:input.emailID}));return;}
    if(!['status','cancel','file'].includes(action) || Object.keys(input).some(key=>!['id','token'].includes(key)) || typeof input.id!=='string' || !/^[1-9][0-9]{0,18}$/.test(input.id) || typeof input.token!=='string' || !/^[A-Za-z0-9_-]{43}$/.test(input.token))throw new MailControlError(400);
    const result=action==='file'?await fileQueuedWebmail(control,store,{id:input.id,token:input.token},options.clientFactory):await control({action:'send.webmail.status',id:input.id,token:input.token,cancel:action==='cancel'}) as {id:string;state:string;filed:boolean;filingState?:string;cleanupRef?:string};
    if('cleanupRef' in result && typeof result.cleanupRef==='string')await store.remove(result.cleanupRef);
    reply(response,200,{id:result.id,state:result.state,filed:result.filed,filingState:'filingState' in result?result.filingState:undefined});return;
   }
   const policy=/^\/internal\/smtp\/([1-9][0-9]{0,18})$/.exec(request.url??'');
   if(policy && request.method==='POST'){await submissionPolicy(request,response,policy[1],control);return;}
   const token=request.headers.authorization?.match(/^Bearer (wd_mail_[A-Za-z0-9_-]{43})$/)?.[1];
   if(!token)throw new MailControlError(401);
   const path=request.url??'';
   const existing=/^\/v1\/emails\/([1-9][0-9]{0,18})$/.exec(path);
   if(existing && ['GET','DELETE'].includes(request.method??'')){
    const result=await control({action:'send.status',token,id:existing[1],cancel:request.method==='DELETE'}) as {id:string;state:string;cleanupRef?:string};
    if(result.cleanupRef)await store.remove(result.cleanupRef);
    reply(response,200,{id:result.id,state:result.state});return;
   }
   if(path!=='/v1/emails' || request.method!=='POST'){reply(response,404,{error:'Not found.'});return;}
   const header=request.headers['idempotency-key'];
   if(header!==undefined && (typeof header!=='string' || !/^[\x21-\x7e]{1,200}$/.test(header)))throw new MailControlError(400);
   // Authenticate before consuming a large body; one upload per customer prevents a single key occupying every slot.
   const access=await control({action:'send.authenticate',token}) as {customerID:string};
   if(!access || !/^[1-9][0-9]{0,18}$/.test(access.customerID))throw new MailControlError(503);
   if(customers.has(access.customerID))throw new MailControlError(429);customer=access.customerID;customers.add(customer);
   let message;
   try{message=parseSendRequest(await readJSON(request));}catch(error){if(error instanceof MailControlError)throw error;throw new MailControlError(400);}
   try{await compileSendRequest(message);}catch{throw new MailControlError(413);}
   const envelope={from:message.from.email,to:[...message.to,...message.cc??[],...message.bcc??[]]};
   const payload=Buffer.from(JSON.stringify(message)),ref=randomUUID();
   const result=await control({action:'send.admit',submission:{token,idempotencyKey:header??randomUUID(),requestHash:createHash('sha256').update(payload).digest('hex'),payloadRef:ref,payloadBytes:payload.length,envelope,...(message.scheduled_at?{notBefore:message.scheduled_at}:{})}}) as {id:string;state:string;created:boolean;uploadToken?:string;customerID?:string;payloadRef?:string};
   if(!result || !/^[1-9][0-9]{0,18}$/.test(result.id) || typeof result.created!=='boolean')throw new MailControlError(503);
   submissionID=result.id;
   if(!result.created){reply(response,result.state==='staging'?409:202,{id:result.id,state:result.state});return;}
   if(result.state!=='staging' || result.customerID!==customer || result.payloadRef!==ref || !/^[a-f0-9-]{36}$/.test(result.uploadToken??''))throw new MailControlError(503);
   const lease={id:result.id,customerID:customer,payloadRef:ref,uploadToken:result.uploadToken};
   try{await store.put(customer,payload,ref);}catch(error){await control({action:'send.abortPayload',lease}).catch(()=>{});throw error;}
   // Lost acknowledgement leaves a tracked staging/queued payload, never an unowned file or a resend.
   const committed=await control({action:'send.commitPayload',lease}) as {queued?:boolean};
   if(committed?.queued!==true)throw new MailControlError(503);
   reply(response,202,{id:result.id,state:'queued'});
  }catch(error){reply(response,error instanceof MailControlError?error.status:503,{error:'Mail request could not be completed.',...(submissionID?{id:submissionID}:{})});}
  finally{if(customer)customers.delete(customer);active--;}
 });
 server.headersTimeout=10000;server.requestTimeout=20000;server.keepAliveTimeout=5000;server.maxHeadersCount=40;server.maxConnections=64;
 return server;
}
