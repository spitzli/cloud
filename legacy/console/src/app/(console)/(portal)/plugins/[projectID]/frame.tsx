'use client';
import {useEffect,useRef,useState} from 'react';
import {invokePlugin} from './actions';
import {pluginPanelURL} from '@/lib/plugin-panel';
import {createPluginFrameBudget} from '@/lib/plugin-frame-budget';
type Action={name:string;intent:'read'|'write';destructive?:boolean};
export function PluginFrame({pluginID,projectID,url,actions,label,allowLocalHTTP=false}:{pluginID:string;projectID:string;url:string;actions:Action[];label:string;allowLocalHTTP?:boolean}){
 const container=useRef<HTMLDivElement>(null);const [error,setError]=useState('');
 useEffect(()=>{
  if(!container.current)return;
  const origin=new URL(url);if(!pluginPanelURL(origin.origin,origin.pathname+origin.search,{allowLocalHTTP})){setError('Plugin UI requires HTTPS.');return;}
  const iframe=document.createElement('iframe');iframe.title=label;iframe.setAttribute('sandbox','allow-scripts');iframe.referrerPolicy='no-referrer';iframe.style.cssText='width:100%;height:740px;border:1px solid var(--border,#ddd);border-radius:8px;background:white';
  const nonce=crypto.randomUUID();iframe.src=url+'#'+nonce;let channel:MessageChannel|undefined;let alive=true;const budget=createPluginFrameBudget();
  iframe.onload=()=>{
   channel?.port1.close();channel=new MessageChannel();const port=channel.port1;
   port.onmessage=async event=>{
    const m=event.data;if(!alive||!m||typeof m.id!=='string'||m.type!=='action')return;
    let bytes;try{bytes=new TextEncoder().encode(JSON.stringify(m)).length;}catch{return;}if(bytes>32768)return;
    const action=actions.find(a=>a.name===m.name);if(!action||!budget.reserve(m.id,bytes))return;
    if(action.destructive&&!window.confirm(`Allow ${label} to perform ${action.name}?`)){budget.release(m.id);port.postMessage({id:m.id,error:'Cancelled'});return;}
    try{const response=await invokePlugin({pluginID,projectID,action:action.name,input:m.input,idempotencyKey:m.id});if(alive)port.postMessage({id:m.id,...response});}catch{if(alive)port.postMessage({id:m.id,error:'Plugin unavailable'});}finally{budget.release(m.id);}
   };
   // Sandboxed frames have opaque origins. Transfer this private port only to the exact frame window.
   iframe.contentWindow?.postMessage({type:'webdock:init',nonce},'*',[channel.port2]);
  };
  container.current.replaceChildren(iframe);
  return()=>{alive=false;channel?.port1.close();iframe.remove();};
 },[pluginID,projectID,url,actions,label,allowLocalHTTP]);
 return <>{error&&<p role="alert">{error}</p>}<div ref={container}/></>;
}
