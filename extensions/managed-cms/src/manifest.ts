import { parsePluginManifest } from "@webdock/plugin-sdk/contracts";
const object=(properties:Record<string,unknown>,required=Object.keys(properties))=>({type:"object",additionalProperties:false,properties,required});
const string=(maxLength:number,minLength=1)=>({type:"string",minLength,maxLength});
export function createManifest(image:string){
 const actions=[
  {name:"cms.instances",intent:"read",capabilities:["plugin.apps.read"],input:object({})},
  {name:"cms.status",intent:"read",capabilities:["plugin.apps.read"],input:object({provisioning_id:string(20)})},
  {name:"cms.create",intent:"write",capabilities:["plugin.apps.provision"],input:object({name:string(160),request_id:string(128)})},
  {name:"cms.read",intent:"read",httpBinding:"cms",capabilities:["plugin.http.read"],input:object({path:string(2048)})},
  {name:"cms.write",intent:"write",httpBinding:"cms",capabilities:["plugin.http.request"],input:object({op:{type:"string",enum:["begin","append","commit"]},transfer:string(128),size:{type:"integer",minimum:1,maximum:15728640},sha256:string(64),kind:{type:"string",enum:["json","media"]},offset:{type:"integer",minimum:0,maximum:15728640},data:string(21848,0)},["op"])},
 ];
 return parsePluginManifest({name:"managed-cms",version:"0.1.0-alpha.1",kind:"extension",description:"Create independent website CMS instances and edit pages, drafts and media in Webdock.",license:"Apache-2.0",category:"content",tags:["cms","content","websites"],image,
  protocolVersion:1,sdkVersion:1,dataProtocolVersion:1,coreVersion:">=0.1.0 <1.0.0",schemaVersion:1,readableSchema:{min:1,max:1},
  capabilities:["plugin.apps.read","plugin.apps.provision","plugin.http.read","plugin.http.request"],services:{required:[{name:"http",version:1}],optional:[{name:"apps",version:1}]},httpBindings:["cms"],
  configuration:object({}),actions,navigation:[{id:"cms",label:"CMS",path:"/ui/cms"}],api:[],mcp:[],jobs:[],events:[],slots:[],tables:[],migrations:[],dependencies:[],secrets:[],
  homepage:"https://github.com/spitzli/webdock/tree/main/extensions/managed-cms",documentation_url:"https://github.com/spitzli/cloud-plugin-managed-cms/blob/HEAD/README.md",readme:"README.md",icon:"assets/icon.svg",screenshots:[{"url":"screenshots/cms-draft-editor.png","caption":"Actual CMS editor with synthetic draft content and a read-only fixture transport. No content was saved or published.","alt":"CMS page editor with title, page address, introduction and draft controls"},{"url":"screenshots/cms-draft-mobile.png","caption":"Actual CMS editor at 320px with synthetic content; no save or publish operation.","alt":"Responsive CMS page editor and Spitzli Cloud wordmark at 320px"}],
 });
}
