import 'server-only';
import {studioCall} from './studio-client';
export type MailConnection={id:string;kind:'platform'|'byok';region:'eu'|'global';state:string;revision:string;created_at:string};
export type MailRow={id:string;[key:string]:unknown};
export type MailPage={data:MailRow[];nextCursor:string|null};
export type MailMessage=MailRow&{from_address:string;subject:string;dispatch_status:string;created_at:string;revision:string;scheduled_at:string|null;content_available:boolean;content:{text:string|null;html:string|null;attachments:{filename?:string;size:number}[]}|null;recipients:{email:string;category:string;delivery_status:string;suppression_reason:string|null}[];events:{id:string;type:string;recipient:string|null;occurred_at:string;source:string}[]};
export type MailAnalytics={messagesAccepted:number;recipientsAccepted:number;delivered:number;bounced:number;suppressed:number;opened:number;clicked:number;deliveryRate:number|null;openRate:number|null;clickRate:number|null;simulatedIncluded:boolean;days:{date:string;accepted:number;delivered:number;bounced:number}[]};
export function mailCall<T>(operation:string,customerID:string,...args:unknown[]):Promise<T>{return studioCall<T>(operation,[customerID,...args]);}

export async function mailRead<T>(operation:string,customerID:string,...args:unknown[]):Promise<T>{try{return await mailCall<T>(operation,customerID,...args);}catch(error){const {StudioError}=await import('./studio-errors');if(error instanceof StudioError&&[403,404].includes(error.status)){const {notFound}=await import('next/navigation');notFound();}throw error;}}
export type MailHealth={gatewayEnabled:boolean;dispatchEnabled:boolean;webhooksEnabled:boolean;jobs:{name:string;status:string;last_success:string|null}[];queue:{unknown:number;blocked:number;oldest_due:string|null};callbacks:{configured:boolean;last_event:string|null};credentials:{configured:boolean};outbox:{pending:number;oldest_due:string|null};inbox:{pending:number;quarantined:number;oldest_due:string|null}|null;alerts:{inboxLate:boolean;tickLate:boolean;dueLate:boolean;unknownLate:boolean}};
