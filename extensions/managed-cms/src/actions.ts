import { z } from "zod";
import type { PluginApps, PluginHTTP } from "@spitzli/plugin-sdk/contracts";
const empty = z.strictObject({});
const transfer = z.string().regex(/^[a-zA-Z0-9_-]{1,128}$/);
const write = z.discriminatedUnion("op", [
  z.strictObject({op:z.literal("begin"),size:z.number().int().min(1).max(15*1024*1024),sha256:z.string().regex(/^[a-f0-9]{64}$/),kind:z.enum(["json","media"])}),
  z.strictObject({op:z.literal("append"),transfer,offset:z.number().int().min(0).max(15*1024*1024),data:z.string().max(21848).regex(/^[A-Za-z0-9+/]*={0,2}$/)}),
  z.strictObject({op:z.literal("commit"),transfer}),
]);
const read = z.strictObject({path:z.string().max(2048).refine(value=>{
  if(!value.startsWith("/api/webdock/cms?") || /[%#\\\x00-\x20]/.test(value))return false;
  const url=new URL(value,"https://instance.invalid"),keys=[...url.searchParams.keys()];
  return (keys.length===1 && keys[0]==="request" && /^[A-Za-z0-9_-]+$/.test(url.searchParams.get("request")!)) ||
    (keys.length===2 && keys.includes("transfer") && keys.includes("offset") && transfer.safeParse(url.searchParams.get("transfer")).success && /^\d{1,8}$/.test(url.searchParams.get("offset")!));
})});
const create=z.strictObject({name:z.string().trim().min(1).max(160),request_id:z.string().regex(/^[A-Za-z0-9_-]{1,128}$/)});
const status=z.strictObject({provisioning_id:z.string().regex(/^[0-9]{1,20}$/)});
export async function runAction(context:{apps:PluginApps;http:PluginHTTP},action:string,input:unknown) {
  switch(action){
    case "cms.instances": empty.parse(input); return context.apps.provisioningList();
    case "cms.create": return context.apps.provision({template_id:"managed-cms",...create.parse(input)});
    case "cms.status": return context.apps.provisioningStatus(status.parse(input));
    case "cms.read": return context.http.request({binding:"cms",method:"GET",...read.parse(input)});
    case "cms.write": return context.http.request({binding:"cms",method:"POST",path:"/api/webdock/cms",body:write.parse(input)});
    default: throw Error("Unknown CMS action");
  }
}
