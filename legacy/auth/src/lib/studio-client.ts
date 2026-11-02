import 'server-only';
import {headers} from 'next/headers';
import {requestAPI} from '@webdock/api-client';
import {StudioError,AccessError,studioError} from './studio-errors';
export type StudioSession={user:{id:string;name:string;email:string;role:'operator'|'user';emailVerified:boolean;twoFactorEnabled:boolean;mustChangePassword:boolean};operator:boolean};
export type AuthSession={user:StudioSession['user'] & {banned?:boolean};session:{id:string;userId:string;expiresAt:string}};
export async function authView<T>(path:string,body?:unknown,requestHeaders?:Headers):Promise<T>{
 const incoming=requestHeaders||await headers();
 if(!process.env.WEBDOCK_API_ORIGIN)throw new StudioError('Account API is not configured.',503);
 let response:{status:number;body:{data?:T;error?:{kind?:string;message?:string}}};
 try{response=await requestAPI({apiOrigin:process.env.WEBDOCK_API_ORIGIN,uiOrigin:process.env.BETTER_AUTH_URL||'https://auth.webdock.dev',cookie:incoming.get('cookie')||'',path,body});}
 catch{throw new StudioError('Account services are temporarily unavailable.',503);}
 if(response.status===401)throw new AccessError('Sign in again.',401);
 if(response.status<200||response.status>=300||response.body.error)throw studioError(response.body.error?.kind||'StudioError',response.body.error?.message||'Request unavailable.',response.status);
 return response.body.data as T;
}
export function studioCall<T>(operation:string,args:unknown[]=[]):Promise<T>{return authView('/v1/auth/studio',{operation,args});}
export const getStudioSession=()=>studioCall<StudioSession>('session');
