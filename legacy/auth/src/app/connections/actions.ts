'use server';
import {revalidatePath} from 'next/cache';
import {studioCall} from '@/lib/studio-client';
export type ClientState={error?:string;clientID?:string;clientSecret?:string};
export async function registerClient(_state:ClientState,form:FormData):Promise<ClientState>{
 try{const input=Object.fromEntries([...form].filter((entry):entry is [string,string]=>typeof entry[1]==='string'));
 const result=await studioCall<ClientState>('registerConnection',[input]);revalidatePath('/connections');return result;}
 catch{return{error:'The client could not be registered. Check your operator session and try again.'};}
}
export async function disableClient(form:FormData){await studioCall('disableConnection',[String(form.get('id')||'')]);revalidatePath('/connections');}
