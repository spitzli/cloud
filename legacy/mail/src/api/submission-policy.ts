import type {IncomingMessage,ServerResponse} from 'node:http';
import {z} from 'zod';
import {hookMessageDigest} from '@webdock/mail-core/submission-proof';
import type {MailControl} from './server.ts';
const address=z.object({address:z.string().max(254)});
const hook=z.object({context:z.object({stage:z.literal('data'),sasl:z.object({login:z.string().max(320)}),queue:z.object({id:z.string().regex(/^[a-f0-9]{1,32}$/)})}),envelope:z.object({from:address,to:z.array(address).min(1).max(100)}),message:z.object({headers:z.array(z.tuple([z.string().max(256),z.string().max(131072)])).max(1000),contents:z.string(),size:z.number().int().positive().max(26214400)})});
/** Cluster-only endpoint: public ingress must not expose /internal/. Auth independently verifies its tenant token. */
export async function submissionPolicy(request:IncomingMessage,response:ServerResponse,customerID:string,control:MailControl){
 response.setHeader('Content-Type','application/json');response.setHeader('Cache-Control','no-store');
 const token=request.headers.authorization?.match(/^Bearer (wd_hook_[1-9][0-9]{0,18}_[A-Za-z0-9_-]{43})$/)?.[1];
 if(!token?.startsWith(`wd_hook_${customerID}_`)){response.writeHead(401);response.end('{}');return;}
 const chunks:Buffer[]=[];let size=0;const timer=setTimeout(()=>request.destroy(),20000);
 try{
  const authenticated=await control({action:'send.policy.auth',customerID,token}) as {ok?:boolean};
  if(authenticated?.ok!==true)throw Error('Policy token is unavailable');
  for await(const chunk of request){size+=chunk.length;if(size>52*1024*1024)throw Error('Oversized hook');chunks.push(Buffer.from(chunk));}
  const message=hook.parse(JSON.parse(Buffer.concat(chunks).toString('utf8')));
  const proofs=message.message.headers.filter(([name])=>name.toLowerCase()==='x-webdock-submission');
  const proof=proofs.length===1?/^([1-9][0-9]{0,18})\.([A-Za-z0-9_-]{43})$/.exec(proofs[0][1].trim()):null;
  let accepted=false;
  if(proof){
   const result=await control({action:'send.policy',submission:{customerID,token,id:proof[1],proof:proof[2],digest:hookMessageDigest(message.message),username:message.context.sasl.login,queueID:message.context.queue.id,envelope:{from:message.envelope.from.address,to:message.envelope.to.map(item=>item.address)}}}) as {accept?:boolean};
   accepted=result?.accept===true;
  }
  response.end(JSON.stringify(accepted?{action:'accept',modifications:[{type:'deleteHeader',index:1,name:'X-Webdock-Submission'}]}:{action:'reject',response:{status:550,enhanced_status:'5.7.1',message:'Webdock submission is not authorized.'}}));
 }catch{response.writeHead(503);response.end('{}');}
 finally{clearTimeout(timer);}
}
