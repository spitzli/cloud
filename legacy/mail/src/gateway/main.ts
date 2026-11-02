import { createMailGateway } from "./server.ts";
const required = (name: string) => { const value=process.env[name];if(!value)throw Error(`${name} is required`);return value; };
const authOrigin=new URL(required("WEBDOCK_AUTH_ORIGIN"));
if(authOrigin.protocol!=="https:" || authOrigin.username || authOrigin.password || authOrigin.pathname!=="/" || authOrigin.search || authOrigin.hash)throw Error("Mail control plane requires an HTTPS origin");
const secret=required("MAIL_GATEWAY_SECRET");
if(secret.length<32 || secret.length>512 || /\s/.test(secret))throw Error("Invalid Mail gateway secret");
const sendOrigin=new URL(process.env.MAIL_SEND_INTERNAL_ORIGIN||'http://127.0.0.1:3139');
if(sendOrigin.protocol!=='http:' || sendOrigin.hostname!=='127.0.0.1' || sendOrigin.username || sendOrigin.password || sendOrigin.pathname!=='/' || sendOrigin.search || sendOrigin.hash)throw Error('Mail sending must use the same-pod private endpoint.');
const server=createMailGateway({nativeSending:async body=>{
 const response=await fetch(new URL('/internal/webmail',sendOrigin),{method:'POST',redirect:'error',signal:AbortSignal.timeout(30000),headers:{'Content-Type':'application/json',Authorization:'Bearer '+secret},body:JSON.stringify(body)});
 if(!response.ok || !response.body){await response.body?.cancel();throw Error('Mail submission is unavailable.');}
 const reader=response.body.getReader(),parts:Uint8Array[]=[];let size=0;
 try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>4096)throw Error('Mail submission response is too large.');parts.push(part.value);}}
 finally{await reader.cancel().catch(()=>{});reader.releaseLock();}
 return JSON.parse(Buffer.concat(parts).toString('utf8'));
},allowedOrigins:required("MAIL_BROWSER_ORIGINS").split(","),control:async body=>{
  const response=await fetch(new URL("/api/mail/gateway",authOrigin),{method:"POST",redirect:"error",signal:AbortSignal.timeout(10_000),headers:{"Content-Type":"application/json",Authorization:"Bearer "+secret},body:JSON.stringify(body)});
  if(!response.ok || !response.body){await response.body?.cancel();throw Error("Mail authorization unavailable");}
  const reader=response.body.getReader(),parts:Uint8Array[]=[];let size=0;
  try{for(;;){const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>65536)throw Error("Mail authorization unavailable");parts.push(part.value);}}
  finally{await reader.cancel().catch(()=>{});reader.releaseLock();}
  const data=JSON.parse(Buffer.concat(parts).toString());return data.data;
}});
const port=Number(process.env.PORT || 3138);
if(!Number.isSafeInteger(port) || port<1024 || port>65535)throw Error("Invalid Mail gateway port");
server.listen(port,process.env.HOST || "127.0.0.1",()=>console.log("Webdock Mail gateway listening"));
for(const signal of ["SIGINT","SIGTERM"] as const)process.once(signal,()=>{
  server.close(()=>process.exit(0));
  setTimeout(()=>{server.closeAllConnections();process.exit(1);},30_000).unref();
});
