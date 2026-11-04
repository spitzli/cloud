/** Synthetic acceptance extension: a separate private-TLS process, never a production plugin. */
import {readFileSync} from 'node:fs';
import {createPluginContext,verifyInvocation} from '@spitzli/plugin-sdk/server';
const c=JSON.parse(readFileSync(process.argv[2],'utf8'));
const html=`<!doctype html><html><head><title>Synthetic acceptance notes</title></head><body><h1>Synthetic acceptance notes</h1><p>This local fixture tests the shared extension bridge.</p><label>Project note <input id="note" aria-label="Project note"></label><button id="save" disabled>Save note</button><button id="load" disabled>Load note</button><pre id="result">Waiting for Console bridge</pre><script>
let port;const pending=new Map();window.addEventListener('message',e=>{if(e.source!==parent||e.data?.type!=='webdock:init'||e.data.nonce!==location.hash.slice(1)||!e.ports[0])return;port=e.ports[0];port.onmessage=e=>{const cb=pending.get(e.data.id);if(cb){pending.delete(e.data.id);cb(e.data)}};document.querySelector('#save').disabled=false;document.querySelector('#load').disabled=false;document.querySelector('#result').textContent='Console bridge ready';});
function act(name,input){const id=crypto.randomUUID();pending.set(id,m=>{document.querySelector('#result').textContent=JSON.stringify(m,null,2);if(m.result?.value?.note)document.querySelector('#note').value=m.result.value.note;});port.postMessage({type:'action',id,name,input});}
document.querySelector('#save').onclick=()=>act('notes.write',{note:document.querySelector('#note').value});document.querySelector('#load').onclick=()=>act('notes.read',{});
</script></body></html>`;
Bun.serve({hostname:'127.0.0.1',port:3446,fetch(request){return request.method==='GET'&&new URL(request.url).pathname==='/ui/panel'?new Response(html,{headers:{'content-type':'text/html','cache-control':'no-store'}}):new Response(null,{status:404});}});
Bun.serve({hostname:'127.0.0.1',port:3443,tls:{cert:c.cert,key:c.key},async fetch(request){
const path=new URL(request.url).pathname;
if(path==='/healthz'){if(request.headers.get('authorization')!=='Bearer '+c.credential)return new Response(null,{status:403});return Response.json({pluginID:c.id,version:'1.0.0',generation:1,protocolVersion:1});}
try{const token=request.headers.get('x-plugin-invocation')!;verifyInvocation(token,{publicKey:c.publicKey,pluginID:c.id,generation:1});const body=await request.json();const ctx=createPluginContext({hostURL:'https://localhost:3444',token:()=>token,runtimeCredential:c.credential,fetch:((url:any,options:any)=>fetch(url,{...options,tls:{ca:c.cert}})) as typeof fetch});
if(path.endsWith('/notes.read'))return Response.json(await ctx.data.read('note'));
if(path.endsWith('/notes.write')){const current=await ctx.data.read('note');return Response.json(await ctx.data.write({key:'note',value:body.input,expectedRevision:current.revision,idempotencyKey:body.idempotencyKey??crypto.randomUUID()}));}
return new Response(null,{status:404});}catch(e){return Response.json({error:String(e)},{status:400});}
}});
Bun.serve({hostname:'127.0.0.1',port:3444,tls:{cert:c.cert,key:c.key},async fetch(request){const url=new URL(request.url);url.protocol='http:';url.hostname='127.0.0.1';url.port='3463';const headers=new Headers(request.headers);headers.set('host','localhost:3463');return fetch(new Request(url,{method:request.method,headers,body:request.body,duplex:'half'} as any));}});
console.log('Synthetic acceptance runtime ready');
