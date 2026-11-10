import { readFileSync } from "node:fs";
import { createRuntimeHandler } from "./runtime";
const required=(name:string)=>{const value=process.env[name];if(!value)throw Error(`${name} is required`);return value;};
const consoleOrigin=required("SPITZLI_PLUGIN_CONSOLE_ORIGIN");
if(new URL(consoleOrigin).origin!==consoleOrigin||!consoleOrigin.startsWith("https://"))throw Error("Canonical HTTPS Console origin required");
Bun.serve({hostname:"0.0.0.0",port:3443,maxRequestBodySize:32768,
 tls:{cert:readFileSync(required("SPITZLI_PLUGIN_TLS_CERT")),key:readFileSync(required("SPITZLI_PLUGIN_TLS_KEY"))},
 fetch:createRuntimeHandler({credential:required("SPITZLI_PLUGIN_RUNTIME_TOKEN"),hostOrigin:required("SPITZLI_PLUGIN_HOST_URL"),publicKey:required("SPITZLI_PLUGIN_PUBLIC_KEY"),generation:Number(required("SPITZLI_PLUGIN_GENERATION"))}),
});
const assets:Record<string,[string,string]>={"/ui/cms":["index.html","text/html"],"/ui/cms.js":["cms.js","application/javascript"],"/ui/cms.css":["cms.css","text/css"]};
Bun.serve({hostname:"0.0.0.0",port:3444,maxRequestBodySize:1024,fetch(request){
 const entry=assets[new URL(request.url).pathname];if(request.method!=="GET"||!entry)return new Response("Not found",{status:404});
 return new Response(Bun.file(new URL("./ui/"+entry[0],import.meta.url)),{headers:{
  "content-type":entry[1]+"; charset=utf-8","cache-control":"no-cache","x-content-type-options":"nosniff","referrer-policy":"no-referrer",
  "content-security-policy":`default-src 'none'; script-src 'self'; style-src 'self'; img-src https: data: blob:; frame-ancestors ${consoleOrigin}; base-uri 'none'; form-action 'none'; connect-src 'none'`,
 }});
}});
