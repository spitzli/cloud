'use server';
import {redirect} from 'next/navigation';
import {msgid} from '@webdock/i18n';
import {revalidatePath} from 'next/cache';import {mailCall} from '@/lib/mail-product';import {StudioError} from '@/lib/studio-errors';
export type MailActionState={error?:string;message?:string;id?:string;key?:string;secret?:string;uncertain?:boolean};
export async function mailProductAction(_previous:MailActionState,form:FormData):Promise<MailActionState>{
 const value=(name:string)=>typeof form.get(name)==='string'?String(form.get(name)):'';
 const customer=value('customer'),connection=value('connection'),operation=value('operation'),id=value('id');let args:unknown[];
 switch(operation){
  case 'mailTestSend':args=[connection,{from:value('from'),to:value('to'),subject:value('subject'),text:value('text'),...(value('scheduled_at')?{scheduled_at:value('scheduled_at')}:{ }),idempotencyKey:value('idempotencyKey')}];break;
  case 'mailDomainCreate':args=[connection,value('name')];break;
  case 'mailReconcile':case 'mailDomainVerify':case 'mailDomainRemove':case 'mailKeyRevoke':case 'mailWebhookDelete':case 'mailWebhookRotate':case 'mailWebhookReplay':case 'mailSuppressionRemove':args=[connection,id];break;
  case 'mailKeyCreate':args=[connection,{name:value('name'),permission:value('permission'),...(value('domainID')?{domainID:value('domainID')}:{})}];break;
  case 'mailWebhookCreate':args=[connection,{url:value('url'),events:form.getAll('events').map(String)}];break;
  case 'mailWebhookUpdate':args=[connection,id,{enabled:value('enabled')==='true'}];break;
  case 'mailSuppressionSet':args=[connection,{email:value('email'),reason:'manual'}];break;
  case 'mailReschedule':args=[connection,id,value('scheduled_at'),Number(value('revision'))];break;
  case 'mailCancel':args=[connection,id,Number(value('revision'))];break;
  case 'mailConnectionUpdate':args=[connection,{action:value('action'),revision:Number(value('revision'))}];break;
  case 'mailConnectByok':args=[{consumerKey:value('consumerKey'),consumerSecret:value('consumerSecret'),expectedRevision:Number(value('revision'))}];break;
  case 'mailRotateManaged':args=[connection];break;
  case 'mailConfigureManaged':args=[];break;
  default:return {error:msgid('Unknown Mail action.')};
 }
 if(['mailDomainRemove','mailKeyRevoke','mailWebhookDelete','mailSuppressionRemove','mailCancel'].includes(operation)&&value('confirm')!=='yes')return {error:msgid('Confirm this change first.')};
 let destination:string|undefined,saved:MailActionState={};
 try{
  const result=await mailCall<Record<string,unknown>>(operation,customer,...args);
  if(typeof result.status==='number'&&result.body&&typeof result.body==='object'){
   const body=result.body as Record<string,unknown>;
   if(result.status>=400)return {error:String(body.message??'Mail request failed.'),uncertain:body.name==='send_outcome_unknown',...(typeof body.email_id==='string'?{id:body.email_id}:{})};
   return {message:msgid('Mail request accepted.'),...(typeof body.id==='string'?{id:body.id}:{})};
  }
  saved={message:msgid('Mail change saved.'),...(typeof result.id==='string'?{id:result.id}:{}),...(typeof result.key==='string'?{key:result.key}:{}),...(typeof result.secret==='string'?{secret:result.secret}:{})};
  if(operation==='mailWebhookDelete'||operation==='mailDomainRemove')destination=`/tenants/${customer}/mail/${operation==='mailWebhookDelete'?'webhooks':'domains'}?connection=${encodeURIComponent(connection)}`;
 }catch(error){return {error:error instanceof StudioError?error.message:msgid('Mail is temporarily unavailable. Retry with the same request.')};}
 finally{if(/^[1-9][0-9]{0,18}$/.test(customer))revalidatePath(`/tenants/${customer}/mail`,'layout');}
 if(destination)redirect(destination);return saved;
}
