import 'server-only';
import {studioCall} from './studio-client';
export type NativeSendingView={customerID:string;transport:null|{mode:'managed'|'byok';state:'pending'|'ready'|'needs_review'|'disabled';canConfigure?:boolean};keys:{id:string;label:string;suffix:string;revokedAt:string|null}[];domains:{id:string;domain:string}[];apiOrigin:string|null;byokAllowed:boolean};
export function getNativeSending(_headers:Headers,customerID:string):Promise<NativeSendingView|null>{return studioCall('getNativeSending',[customerID]);}
export function manageNativeSending(_headers:Headers,customerID:string,input:Record<string,string>):Promise<{message:string;token?:string}>{return studioCall('manageNativeSending',[customerID,input]);}
