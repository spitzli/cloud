import type { ManagedAppProvisioning } from "@spitzli/plugin-sdk/contracts";
export type CMSSite={name:string;origin:string};
export async function discoverCMS(
 transport:(path:string)=>Promise<Response>,
 list:()=>Promise<{provisionings:ManagedAppProvisioning[]}>,
):Promise<{site:CMSSite|null;instance:ManagedAppProvisioning|null}>{
 let response:Response|undefined;
 try{response=await transport("/api/cms");}catch{/* A new project has no website binding until provisioning completes. */}
 if(response?.ok){
  const manifest=await response.json(),url=new URL(manifest.siteURL);
  if(url.protocol!=="https:"||url.username||url.password||url.pathname!=="/"||url.search||url.hash||typeof manifest.siteName!=="string"||!manifest.siteName||manifest.siteName.length>200)throw Error("The CMS connection has invalid website metadata.");
  return {site:{name:manifest.siteName,origin:url.origin},instance:null};
 }
 const result=await list();
 return {site:null,instance:result.provisionings.find(item=>item.template_id==="managed-cms")??null};
}
