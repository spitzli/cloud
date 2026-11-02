'use server';
import {headers} from 'next/headers';
import {revalidatePath} from 'next/cache';
import {manageNativeSending} from '@/lib/mail-sending';
import {StudioError} from '@/lib/studio-errors';
export type SendingResult={message?:string;error?:string;token?:string};
export async function sendingAction(_state:SendingResult,form:FormData):Promise<SendingResult>{
 const value=(key:string)=>typeof form.get(key)==='string'?String(form.get(key)):'';
 const customer=value('customer');
 try{
  const result=await manageNativeSending(await headers(),customer,{action:value('action'),mode:value('mode'),label:value('label'),domainID:value('domainID'),keyID:value('keyID')});
  revalidatePath(`/tenants/${customer}/mail/sending`);return result;
 }catch(error){return {error:error instanceof StudioError?error.message:'Mail settings could not be changed. Refresh before trying again.'};}
}
