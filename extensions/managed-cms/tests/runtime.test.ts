import { test, expect } from "bun:test";
import { generateKeyPairSync, sign } from "node:crypto";
import { createRuntimeHandler } from "../src/runtime";
const keys=generateKeyPairSync("ed25519");
const config={credential:"a".repeat(40),publicKey:keys.publicKey.export({type:"spki",format:"pem"}).toString(),hostOrigin:"https://api.example.test",generation:1};
const token=(patch:object={})=>{const now=Math.floor(Date.now()/1000);const body=Buffer.from(JSON.stringify({v:1,id:crypto.randomUUID(),pluginID:"managed-cms",generation:1,action:"cms.instances",projectID:"123",customerID:"101",iat:now,exp:now+50,...patch})).toString("base64url");return body+"."+sign(null,Buffer.from(body),keys.privateKey).toString("base64url");};
test("private runtime requires both the runtime credential and signed exact-action invocation",async()=>{
 let calls=0;const handle=createRuntimeHandler(config,()=>({apps:{provisioningList:async()=>{calls++;return{provisionings:[]};}}} as any));
 const request=(credential:string,invocation:string,hostOrigin=config.hostOrigin)=>new Request("https://runtime/actions/cms.instances",{method:"POST",headers:{authorization:credential,"x-plugin-invocation":invocation,"content-type":"application/json"},body:JSON.stringify({input:{},configuration:{},hostOrigin})});
 expect((await handle(request("Bearer wrong",token()))).status).toBe(401);
 expect((await handle(request("Bearer "+config.credential,token({pluginID:"other"})))).status).toBe(403);
 expect((await handle(request("Bearer "+config.credential,token({action:"cms.create"})))).status).toBe(403);
 expect((await handle(request("Bearer "+config.credential,token(),"https://evil.test"))).status).toBe(403);
 expect(calls).toBe(0);
 const response=await handle(request("Bearer "+config.credential,token()));
 expect(response.status).toBe(200);expect(await response.json()).toEqual({provisionings:[]});expect(calls).toBe(1);
});
