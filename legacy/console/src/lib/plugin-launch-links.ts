type LaunchLink = {appID:string;label:string;appURL:string;consolePath:string};
/** Only accept canonical host-derived links; iframe messages never supply navigation. */
export function pluginLaunchLinks(value:unknown):LaunchLink[] {
 if(!Array.isArray(value))return [];
 return value.slice(0,100).flatMap((v:unknown)=>{
  if(!v || typeof v!=='object')return [];
  const l=v as Record<string,unknown>;
  if(typeof l.appID!=='string'||!/^\d{1,20}$/.test(l.appID)||typeof l.label!=='string'||!l.label||l.label.length>200||typeof l.appURL!=='string'||l.consolePath!==`/hosting/apps/${l.appID}`)return [];
  try {
   const url=new URL(l.appURL);
   if(url.protocol!=='https:'||url.origin!==l.appURL||url.username||url.password)return [];
   return [{appID:l.appID,label:l.label,appURL:l.appURL,consolePath:l.consolePath as string}];
  }catch{return [];}
 });
}
