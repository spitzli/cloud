import test from 'node:test';import assert from 'node:assert/strict';import React,{act} from 'react';import {createRoot} from 'react-dom/client';import {Window} from 'happy-dom';import {CMSApp} from './client.tsx';import {gettext} from '@spitzli/i18n';
for(const locale of ['en','de'] as const)test(`an uncertain new save stays visibly blocked until its original content list loads (${locale})`,async()=>{
 const window=new Window({url:'https://site.invalid/cms?module=pages&id=new'});const globals={window,document:window.document,navigator:window.navigator,HTMLElement:window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true};const previous=new Map(Object.keys(globals).map(key=>[key,Object.getOwnPropertyDescriptor(globalThis,key)]));for(const [key,value] of Object.entries(globals))Object.defineProperty(globalThis,key,{value,configurable:true,writable:true});
 const container=window.document.createElement('div');window.document.body.append(container);const root=createRoot(container as never);let mutations=0,listReads=0;
 const module={slug:'pages',kind:'collection',label:'Pages'};const descriptor={...module,fields:[{name:'title',label:'Title',type:'text'}],drafts:true,upload:false,canWrite:true,canCreate:true,canDelete:false};
 const transport=async(input:RequestInfo|URL,init?:RequestInit)=>{
  if(init?.method==='POST'){mutations++;if(mutations===1)throw Object.assign(Error('lost response'),{code:'cms_save_uncertain'});return Response.json({doc:{id:'created'},message:'Draft saved.'})}
  const url=new URL(String(input),'https://site.invalid');if(!url.searchParams.has('module'))return Response.json({siteName:'Fixture',siteURL:'https://site.invalid',user:{name:'Editor',role:'editor'},canWrite:true,canDelete:false,locales:['en'],defaultLocale:'en',modules:[module]});
  if(url.searchParams.has('id'))return Response.json({...descriptor,doc:{title:'Pending title',updatedAt:null}});listReads++;return Response.json({...descriptor,docs:[],totalDocs:0,totalPages:1,page:1});
 };
 const settle=async()=>{for(let count=0;count<5;count++)await act(async()=>{await new Promise(resolve=>setTimeout(resolve,10))})};
 const button=(text:string)=>Array.from(container.querySelectorAll('button')).find(item=>item.textContent===gettext(locale,text))!;
 try{
  await act(async()=>root.render(<CMSApp embedded uiLocale={locale} transport={transport} siteName="Fixture" siteURL="https://site.invalid" accountURL="https://auth.invalid/account" accessURL="https://auth.invalid/people"/>));await settle();assert.ok(button('Save draft'));assert.equal(button('Save draft').type,'button');
  await act(async()=>button('Save draft').click());await settle();assert.equal(mutations,1);assert.ok(container.textContent.includes(gettext(locale,'This save may have completed. Check the content list before trying again.')));assert.equal(button('Save draft').disabled,true);assert.equal(button('Publish').disabled,true);
  await act(async()=>button('Save draft').click());await settle();assert.equal(mutations,1);
  await act(async()=>button('Check saved content').click());await settle();assert.equal(listReads,1);assert.equal(container.textContent.includes(gettext(locale,'This save may have completed. Check the content list before trying again.')),false);assert.equal(mutations,1);
 }finally{await act(async()=>root.unmount());await window.happyDOM.close();for(const [key,value] of previous)if(value)Object.defineProperty(globalThis,key,value);else Reflect.deleteProperty(globalThis,key)}
});
