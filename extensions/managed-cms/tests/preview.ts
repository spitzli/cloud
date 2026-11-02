/** Local integration harness: real CMS fixture + signed runtime, no production account. */
import { generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { createRuntimeHandler } from "../src/runtime";
import { fileURLToPath } from "node:url";
const port=3418,origin=`http://127.0.0.1:${port}`,target="http://127.0.0.1:3148";
const secret=process.env.CMS_BRIDGE_SECRET,instance=process.env.CMS_INSTANCE_ID,project=process.env.CMS_PROJECT_ID;
if(!secret||instance!=="acceptance-binding"||project!=="acceptance-project")throw Error("Use the isolated CMS acceptance fixture environment");
const csrf=randomBytes(24).toString("hex"),keys=generateKeyPairSync("ed25519"),credential=randomBytes(32).toString("base64url");
const config={credential,hostOrigin:"https://fixture-host.invalid",generation:1,publicKey:keys.publicKey.export({type:"spki",format:"pem"}).toString()};
const assets=fileURLToPath(new URL("../dist/ui/",import.meta.url));
const ready={provisioning_id:"123",template_id:"managed-cms",state:"ready",app_id:"456",origin:process.env.CMS_ORIGIN,steps:[]};
function runtime(role:string){return createRuntimeHandler(config,()=>({
 apps:{provisioningList:async()=>({provisionings:[ready]}),provisioningStatus:async()=>ready,provision:async()=>{throw Error("Creation is tested by the Hosting acceptance suite");}} as any,
 http:{request:async input=>{
  if(input.binding!=="cms"||!input.path.startsWith("/api/webdock/cms")||(role==="reader"&&input.method!=="GET"))throw Error("Fixture authority denied");
  const response=await fetch(target+input.path,{method:input.method,headers:{authorization:"Bearer "+secret,"content-type":"application/json","x-webdock-instance":instance!,"x-webdock-project":project!,"x-webdock-subject":"parent-browser-"+role,"x-webdock-role":role,"x-webdock-grant-epoch":"1:0"},...(input.body===undefined?{}:{body:JSON.stringify(input.body)})});
  return {status:response.status,body:await response.json()};
 }},
}));}
Bun.serve({hostname:"127.0.0.1",port,maxRequestBodySize:32768,async fetch(request){
 const url=new URL(request.url);
 if(url.pathname==="/action"&&request.method==="POST"){
  if(request.headers.get("origin")!==origin||request.headers.get("x-cms-preview")!==csrf)return new Response("Denied",{status:403});
  try{
   const input=await request.json();if(!["reader","editor","admin"].includes(input.role))throw Error("Invalid fixture role");
   const now=Math.floor(Date.now()/1000),claims=Buffer.from(JSON.stringify({v:1,id:crypto.randomUUID(),pluginID:"managed-cms",generation:1,action:input.name,projectID:"123",customerID:"101",iat:now,exp:now+50})).toString("base64url");
   const token=claims+"."+sign(null,Buffer.from(claims),keys.privateKey).toString("base64url");
   return runtime(input.role)(new Request("https://runtime.invalid/actions/"+input.name,{method:"POST",headers:{authorization:"Bearer "+credential,"x-plugin-invocation":token,"content-type":"application/json"},body:JSON.stringify({input:input.input,configuration:{},hostOrigin:config.hostOrigin})}));
  }catch{return Response.json({error:"Fixture action failed"},{status:400});}
 }
 if(url.pathname==="/"&&request.method==="GET")return new Response(`<!doctype html><html><head><meta charset="utf-8"><title>CMS integration fixture</title><style>body{margin:0;background:#e9edf1;color:#1b293c;font:15px system-ui}header{padding:16px 24px;display:flex;justify-content:space-between;align-items:center}label{display:flex;align-items:center;gap:10px}select{padding:8px;font:inherit}iframe{display:block;width:100%;height:1000px;border:0;background:white}</style></head><body><header><strong>Webdock · Local CMS integration test</strong><label>Fixture role <select id="role"><option>admin</option><option>editor</option><option>reader</option></select></label></header><div id="slot"></div><script>let channel;function mount(){channel?.port1.close();const role=document.getElementById('role').value,nonce=crypto.randomUUID(),frame=document.createElement('iframe');frame.title='CMS';frame.sandbox='allow-scripts';frame.src='/ui/cms#'+nonce;document.getElementById('slot').replaceChildren(frame);frame.onload=()=>{channel=new MessageChannel();const port=channel.port1;port.onmessage=async event=>{const m=event.data;if(!m||m.type!=='action'||JSON.stringify(m).length>32768)return;try{const response=await fetch('/action',{method:'POST',headers:{'content-type':'application/json','x-cms-preview':${JSON.stringify(csrf)}},body:JSON.stringify({name:m.name,input:m.input,role})});const value=await response.json();port.postMessage(response.ok?{id:m.id,result:value}:{id:m.id,error:value.error??'Action failed'});}catch{port.postMessage({id:m.id,error:'Local test host unavailable'});}};frame.contentWindow.postMessage({type:'webdock:init',nonce},'*',[channel.port2]);};}document.getElementById('role').onchange=mount;mount();</script></body></html>`,{headers:{"content-type":"text/html","cache-control":"no-store"}});
 const files:Record<string,[string,string]>={"/ui/cms":["index.html","text/html"],"/ui/cms.js":["cms.js","application/javascript"],"/ui/cms.css":["cms.css","text/css"]};
 const entry=files[url.pathname];if(entry&&request.method==="GET")return new Response(Bun.file(assets+entry[0]),{headers:{"content-type":entry[1],"content-security-policy":`default-src 'none'; script-src 'self'; style-src 'self'; img-src https: data: blob:; connect-src 'none'; frame-ancestors ${origin}; base-uri 'none'; form-action 'none'`}});
 return new Response("Not found",{status:404});
}});
console.log(`CMS integration preview: ${origin}`);
