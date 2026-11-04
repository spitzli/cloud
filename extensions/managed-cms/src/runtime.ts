import { timingSafeEqual } from "node:crypto";
import { z } from "zod";
import { createPluginContext, verifyInvocation } from "@spitzli/plugin-sdk/server";
import { PluginError } from "@spitzli/plugin-sdk/contracts";
import { runAction } from "./actions";
export type RuntimeConfig={credential:string;hostOrigin:string;publicKey:string;generation:number};
const payloadSchema=z.strictObject({input:z.unknown(),configuration:z.strictObject({}),hostOrigin:z.string().max(2048)});
const json=(body:unknown,status=200)=>Response.json(body,{status,headers:{"cache-control":"no-store","x-content-type-options":"nosniff"}});
export function createRuntimeHandler(config:RuntimeConfig,context:(token:string)=>Parameters<typeof runAction>[0]=(token)=>createPluginContext({hostURL:config.hostOrigin,token:()=>token,runtimeCredential:config.credential})){
 const origin=new URL(config.hostOrigin);
 if(origin.protocol!=="https:"||origin.origin!==config.hostOrigin||!config.publicKey||config.credential.length<32||!Number.isSafeInteger(config.generation)||config.generation<1)throw Error("Invalid plugin runtime configuration");
 return async(request:Request):Promise<Response>=>{
  const url=new URL(request.url);
  if(request.method==="GET"&&url.pathname==="/healthz")return json({ok:true,pluginID:"managed-cms",version:"0.1.0-alpha.1",generation:config.generation,protocolVersion:1});
  const match=/^\/actions\/(cms\.[a-z]+)$/.exec(url.pathname);
  if(request.method!=="POST"||!match||url.search)return json({error:"Not found"},404);
  const supplied=Buffer.from(request.headers.get("authorization")??""),expected=Buffer.from("Bearer "+config.credential);
  if(supplied.length!==expected.length||!timingSafeEqual(supplied,expected))return json({error:"Unauthorized runtime"},401);
  try{
   const token=request.headers.get("x-plugin-invocation")??"";
   const claims=verifyInvocation(token,{publicKey:config.publicKey,pluginID:"managed-cms",generation:config.generation});
   if(claims.action!==match[1])return json({error:"Invocation mismatch"},403);
   const reader=request.body?.getReader(),chunks:Uint8Array[]=[];let bytes=0;
   if(reader)try{for(;;){const part=await reader.read();if(part.done)break;bytes+=part.value.length;if(bytes>32768){await reader.cancel();return json({error:"Request exceeds limit"},413);}chunks.push(part.value);}}finally{reader.releaseLock();}
   const payload=payloadSchema.parse(JSON.parse(Buffer.concat(chunks).toString("utf8")));
   if(payload.hostOrigin!==config.hostOrigin)return json({error:"Invocation mismatch"},403);
   const result=await runAction(context(token),match[1],payload.input);
   if(Buffer.byteLength(JSON.stringify(result))>32768)return json({error:"Response exceeds limit"},502);
   return json(result);
  }catch(error){
   if(error instanceof PluginError)return json({error:error.message},error.code==="permission_denied"?403:409);
   return json({error:"CMS request failed. Refresh status before trying again."},400);
  }
 };
}
