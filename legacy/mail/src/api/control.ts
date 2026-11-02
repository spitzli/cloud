import {MailControlError,type MailControl} from './server.ts';
export function mailControlClient(origin:string,secret:string):MailControl{
 const target=new URL(origin);
 if(target.protocol!=='https:' || target.username || target.password || target.pathname!=='/' || target.search || target.hash || secret.length<32 || secret.length>512 || /\s/.test(secret))throw Error('Invalid Mail control configuration.');
 return async body=>{
  const response=await fetch(new URL('/api/mail/gateway',target),{method:'POST',redirect:'error',signal:AbortSignal.timeout(15000),headers:{'Content-Type':'application/json',Authorization:'Bearer '+secret},body:JSON.stringify(body)});
  if(!response.ok){void response.body?.cancel().catch(()=>{});throw new MailControlError([400,401,403].includes(response.status)?response.status:503);}
  if(!response.body)throw new MailControlError(503);
  const reader=response.body.getReader();const chunks:Uint8Array[]=[];let size=0;
  try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>131072)throw new MailControlError(503);chunks.push(part.value);}}
  finally{void reader.cancel().catch(()=>{});reader.releaseLock();}
  const data=JSON.parse(Buffer.concat(chunks).toString('utf8'));if(!data || !('data' in data))throw new MailControlError(503);return data.data;
 };
}
