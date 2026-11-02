import {envelopeAddress,prepareMessage} from './message.ts';

// Verified contract: https://serversmtp.com/turbo-api/turbo-smtp.yaml (2026-10-11).
// reference_id is correlation, NOT provider idempotency. The durable caller owns the one-attempt fence.
export class RelayAPIFailure extends Error {
 readonly uncertain:boolean;
 constructor(uncertain:boolean){super(uncertain?'Mail handoff needs reconciliation.':'Mail handoff was not accepted.');this.uncertain=uncertain;}
}
type Account={region:'eu'|'global';consumerKey:string;consumerSecret:string};
type Message={referenceID:string;sender:{email:string;name?:string};to:string[];cc?:string[];bcc?:string[];subject:string;raw:Buffer};

/** Internal transport only: caller must bind current sender authorization, budget and account revision. */
export async function submitRelayAPI(account:Account,message:Message,send:typeof fetch=fetch){
 const invalid=()=>{throw new RelayAPIFailure(false);};
 if(!['eu','global'].includes(account.region) || ![account.consumerKey,account.consumerSecret].every(v=>typeof v==='string' && v.length>0 && v.length<=4096 && !/[\x00-\x20\x7f]/.test(v)))invalid();
 if(!/^[1-9][0-9]{0,18}$/.test(message.referenceID) || typeof message.subject!=='string' || message.subject.length>700 || /[\x00-\x1f\x7f]/.test(message.subject))invalid();
 const groups=[message.to,message.cc??[],message.bcc??[]];
 if(groups.some(group=>!Array.isArray(group)) || !message.to.length || groups.reduce((total,group)=>total+group.length,0)>100)invalid();
 const seen=new Set<string>();
 const [to,cc,bcc]=groups.map(group=>group.map(envelopeAddress).filter(address=>{if(seen.has(address))return false;seen.add(address);return true;}));
 const prepared=prepareMessage(message.raw,message.sender,[...to,...cc,...bcc]);
 // JSON MIME is UTF-8. Binary attachments must be MIME transfer encoded before this boundary.
 const mime=prepared.raw.toString('utf8');if(!Buffer.from(mime).equals(prepared.raw))invalid();
 const body=JSON.stringify({from:prepared.from,to:to.join(','),...(cc.length?{cc:cc.join(',')}:{}),...(bcc.length?{bcc:bcc.join(',')}:{}),subject:message.subject,mime_raw:mime,reference_id:message.referenceID});
 if(Buffer.byteLength(body)>=24_000_000)invalid();
 const controller=new AbortController();
 let reader:ReadableStreamDefaultReader<Uint8Array>|undefined;
 let timer:ReturnType<typeof setTimeout>|undefined;
 const timeout=new Promise<never>((_,reject)=>{timer=setTimeout(()=>{controller.abort();reject(new RelayAPIFailure(true));},15000);});
 try{
  return await Promise.race([timeout,(async()=>{
   const response=await send(`https://${account.region==='eu'?'api.eu.turbo-smtp.com':'api.turbo-smtp.com'}/api/v2/mail/send`,{
    method:'POST',redirect:'error',cache:'no-store',signal:controller.signal,
    headers:{'content-type':'application/json',consumerKey:account.consumerKey,consumerSecret:account.consumerSecret},body,
   });
   // Only documented rejection classes release the budget; 5xx can follow provider acceptance.
   if(!response.ok){void response.body?.cancel().catch(()=>{});throw new RelayAPIFailure(![400,401,403,413,429].includes(response.status));}
   if(Number(response.headers.get('content-length'))>65536)throw new RelayAPIFailure(true);
   reader=response.body?.getReader();if(!reader)throw new RelayAPIFailure(true);
   const chunks:Uint8Array[]=[];let size=0;
   for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>65536)throw new RelayAPIFailure(true);chunks.push(part.value);}
   const text=Buffer.concat(chunks).toString('utf8');
   // Node 24 reviver context preserves the original integer token without IEEE-754 rounding.
   const data=JSON.parse(text,(key:string,value:unknown,context?:{source?:string})=>key==='mid' && typeof value==='number'?context?.source:value) as {mid?:unknown};
   if(!data || typeof data.mid!=='string' || !/^[1-9][0-9]{0,19}$/.test(data.mid))throw new RelayAPIFailure(true);
   return {providerID:data.mid};
  })()]);
 }catch(error){if(error instanceof RelayAPIFailure)throw error;throw new RelayAPIFailure(true);}
 finally{clearTimeout(timer);controller.abort();void reader?.cancel().catch(()=>{});}
}
