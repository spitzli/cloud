import {pluginLaunchLinks} from "@/lib/plugin-launch-links";
import {pluginPanelURL} from "@/lib/plugin-panel";
import {getStudioSession,studioCall} from '@/lib/studio-client';
import {PluginFrame} from './frame';
import Link from 'next/link';
type Plugin={manifest:{name:string;description?:string;version:string;navigation:Array<{path:string;label:string}>;actions:Array<{name:string;intent:'read'|'write';destructive?:boolean}>};uiOrigin?:string;launchLinks?:unknown};
export default async function ProjectPlugins({params}:{params:Promise<{projectID:string}>}){
 const {projectID}=await params;
 const [plugins,session]=await Promise.all([studioCall<Plugin[]>('plugins.list',[projectID]),getStudioSession()]);
 return <main><Link href={`/projects/${projectID}`}>Back to project</Link><h1>Project extensions</h1>{session?.operator&&<p><Link className="button" href={`/plugins/${encodeURIComponent(projectID)}/install`}>Install plugin</Link></p>}{plugins.length===0?<p>No extensions are enabled for this project.</p>:plugins.map(p=>{const panel=p.manifest.navigation[0];const url=panel ? pluginPanelURL(p.uiOrigin,panel.path,{allowLocalHTTP:process.env.WEBDOCK_PLUGIN_UI_ALLOW_LOCAL_HTTP==='true',environment:process.env.NODE_ENV}):null;return <section className="panel" key={p.manifest.name}><h2>{p.manifest.navigation[0]?.label??p.manifest.name}</h2><p>{p.manifest.description}</p>{pluginLaunchLinks(p.launchLinks).map(link=><div className="workspace-actions" key={link.appID}><span>{link.label}</span><a href={link.appURL} target="_blank" rel="noopener noreferrer">Open app</a><Link href={link.consolePath}>Manage hosting</Link></div>)}{url&&panel?<PluginFrame pluginID={p.manifest.name} projectID={projectID} url={url} actions={p.manifest.actions} label={panel.label} allowLocalHTTP={process.env.WEBDOCK_PLUGIN_UI_ALLOW_LOCAL_HTTP==='true' && process.env.NODE_ENV!=='production'}/>:<p>This extension has no configured panel.</p>}</section>})}</main>;
}
