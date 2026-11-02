import {z} from 'zod';
import nodemailer from 'nodemailer';
import {envelopeAddress,prepareMessage} from './message.ts';
import type {DispatchMessage} from './dispatch.ts';
const address=z.string().max(400).transform(value=>{
 const named=/^([^<>\r\n]{1,80})\s*<([^<>]+)>$/.exec(value.trim());
 if(named)return {email:envelopeAddress(named[2]),name:named[1].trim().replace(/^"|"$/g,'')};
 return {email:envelopeAddress(value.trim())};
});
const addresses=z.union([z.string(),z.array(z.string()).max(100)]).transform(value=>(Array.isArray(value)?value:[value]).map(value=>address.parse(value).email));
const attachment=z.object({filename:z.string().min(1).max(255).regex(/^[^\x00-\x1f\x7f/\\]+$/),content:z.string().max(14_000_000).regex(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/),content_type:z.string().max(100).regex(/^[\w.+-]+\/[\w.+-]+$/).optional(),content_id:z.string().max(200).regex(/^[A-Za-z0-9_.@-]+$/).optional()}).strict();
const schema=z.object({from:address,to:addresses,cc:addresses.optional(),bcc:addresses.optional(),subject:z.string().max(700).regex(/^[^\x00-\x1f\x7f]*$/),text:z.string().max(2_000_000).optional(),html:z.string().max(2_000_000).optional(),attachments:z.array(attachment).max(10).optional(),scheduled_at:z.string().datetime().optional()}).strict();
export type SendRequest=z.output<typeof schema>;
export function parseSendRequest(input:unknown):SendRequest{
 const value=schema.parse(input);
 if(value.text===undefined && value.html===undefined)throw Error('A message body is required.');
 const all=[...value.to,...value.cc??[],...value.bcc??[]];
 if(!value.to.length || all.length>100)throw Error('Choose between 1 and 100 recipients.');
 const seen=new Set<string>();
 const unique=(list:string[])=>list.filter(address=>{if(seen.has(address))return false;seen.add(address);return true;});
 value.to=unique(value.to);value.cc=unique(value.cc??[]);value.bcc=unique(value.bcc??[]);
 if((value.attachments??[]).reduce((sum,item)=>sum+Buffer.from(item.content,'base64').length,0)>10*1024*1024)throw Error('Attachments exceed 10 MiB.');
 if(value.scheduled_at && new Date(value.scheduled_at).getTime()>Date.now()+30*86400000)throw Error('Schedule within 30 days.');
 return value;
}
export async function compileSendRequest(request:SendRequest):Promise<DispatchMessage>{
 const transport=nodemailer.createTransport({streamTransport:true,buffer:true,newline:'windows',disableFileAccess:true,disableUrlAccess:true});
 const info=await transport.sendMail({from:{address:request.from.email,name:request.from.name},to:request.to,cc:request.cc,bcc:request.bcc,subject:request.subject,text:request.text,html:request.html,
  attachments:request.attachments?.map(item=>({filename:item.filename,content:Buffer.from(item.content,'base64'),contentType:item.content_type,cid:item.content_id})),disableFileAccess:true,disableUrlAccess:true});
 if(!Buffer.isBuffer(info.message) || info.message.length>23_000_000)throw Error('Message exceeds its size limit.');
 return {sender:request.from,to:request.to,cc:request.cc,bcc:request.bcc,subject:request.subject,raw:prepareMessage(info.message,request.from,[...request.to,...request.cc??[],...request.bcc??[]]).raw};
}
/** Revalidate authenticated storage without accepting an alternate public request format. */
export function parseStoredRequest(value:unknown){
 const stored=value as SendRequest;
 if(!stored || typeof stored.from?.email!=='string')throw Error('Invalid stored Mail message.');
 const {from,...rest}=stored;
 return parseSendRequest({...rest,from:from.name?`${from.name} <${from.email}>`:from.email});
}
