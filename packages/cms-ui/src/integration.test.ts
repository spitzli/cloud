/* eslint-disable @typescript-eslint/no-explicit-any */
import test from 'node:test';import assert from 'node:assert/strict';import {randomBytes} from 'node:crypto';
import {buildConfig,getPayload} from 'payload';import {postgresAdapter} from '@payloadcms/db-postgres';
import {protectContent,protectUsers} from '../../instance-kit/src/index';
import {handleCMSRequest} from './server';
import {editorData} from './schema';
// Runs only against a disposable local database; without DATABASE_URL the test is skipped.
const db=new URL(process.env.DATABASE_URL??'postgres://127.0.0.1/unset');if(db.hostname!=='127.0.0.1')throw Error('Disposable local DB only');db.pathname='/webdock_cms_test';
const options={siteName:'CMS fixture',siteURL:'http://localhost:3199',modules:[{slug:'pages',kind:'collection' as const,label:'Pages',titleField:'title'},{slug:'settings',kind:'global' as const,label:'Settings'}]};
test('Real CMS edits retain drafts, permissions, versions and stale-write protection',{skip:!process.env.DATABASE_URL},async()=>{
 const p=await getPayload({config:buildConfig({secret:randomBytes(32).toString('hex'),typescript:{autoGenerate:false},admin:{disable:true},db:postgresAdapter({pool:{connectionString:db.toString()},schemaName:'cms_ui_test',push:true}),collections:[protectUsers({slug:'users',auth:true,fields:[{name:'name',type:'text'}]},'operator@example.invalid'),protectContent({slug:'pages',access:{read:({req})=>Boolean(req.user),readVersions:({req})=>Boolean(req.user)},versions:{drafts:true},fields:[{name:'title',type:'text',required:true},{name:'slug',type:'text',required:true},{name:'items',type:'array',fields:[{name:'text',type:'textarea'},{name:'internal',type:'text',admin:{readOnly:true}},{name:'private',type:'text',admin:{hidden:true}}]}]})],globals:[protectContent({slug:'settings',fields:[{name:'name',type:'text'}]})]})});
 try{
  await p.db.pool.query('TRUNCATE cms_ui_test.users,cms_ui_test.pages,cms_ui_test._pages_v CASCADE');
  const tokens:Record<string,string>={},users:Record<string,any>={};
  for(const role of ['operator','editor','reader']){const password=randomBytes(24).toString('base64url'),email=role+'@example.invalid';users[role]=await p.create({collection:'users',data:{name:role,email,password,role},overrideAccess:true,context:{bootstrap:true}} as never);tokens[role]=(await p.login({collection:'users',data:{email,password}})).token!;}
  const call=(method:string,query='',body?:any,role='operator')=>handleCMSRequest(new Request(options.siteURL+'/api/cms'+query,{method,headers:{Origin:options.siteURL,Authorization:'JWT '+tokens[role],'Content-Type':'application/json'},...(body?{body:JSON.stringify(body)}:{})}),options,async()=>p);
  const emptyGlobal=(await (await call('GET','?module=settings')).json()).doc;
  const firstSave=await call('POST','',{module:'settings',mode:'save',updatedAt:emptyGlobal.updatedAt??null,data:{name:'First settings'}});
  assert.equal(firstSave.status,200,await firstSave.clone().text());
  const preservation:any=await p.create({collection:'pages',data:{title:'Preserve nested fields',slug:'preserve',items:[{text:'Before',internal:'keep-internal',private:'keep-private'}]},overrideAccess:true} as never);
  const loaded=await (await call('GET','?module=pages&id='+preservation.id)).json();
  const editable=editorData(loaded.fields,loaded.doc) as any;editable.items[0].text='After';
  const preserved=await call('POST','',{module:'pages',id:String(preservation.id),mode:'publish',updatedAt:loaded.doc.updatedAt,data:editable});
  assert.equal(preserved.status,200,await preserved.clone().text());
  const saved:any=await p.findByID({collection:'pages',id:preservation.id,overrideAccess:true} as never);
  assert.equal(saved.items[0].internal,'keep-internal');assert.equal(saved.items[0].private,'keep-private');
  const created=await call('POST','',{module:'pages',mode:'publish',data:{title:'Live title',slug:'example',items:[]}});assert.equal(created.status,201,await created.clone().text());const doc=(await created.json()).doc;
  const read=async()=> (await (await call('GET','?module=pages&id='+doc.id)).json()).doc;
  let current=await read();
  assert.equal((await call('POST','',{module:'pages',id:String(doc.id),updatedAt:'stale',mode:'draft',data:{title:'No',slug:'example'}})).status,409);
  const draft=await call('POST','',{module:'pages',id:String(doc.id),updatedAt:current.updatedAt,mode:'draft',data:{title:'Draft title',slug:'example',items:[]}},'editor');assert.equal(draft.status,200,await draft.clone().text());
  assert.equal((await p.findByID({collection:'pages',id:doc.id,draft:false,overrideAccess:true} as never) as any).title,'Live title');
  current=await read();assert.equal(current.title,'Draft title');
  assert.equal((await call('POST','',{module:'pages',id:String(doc.id),updatedAt:current.updatedAt,mode:'publish',data:{title:'New live title',slug:'example',items:[]}},'reader')).status,403);
  const published=await call('POST','',{module:'pages',id:String(doc.id),updatedAt:current.updatedAt,mode:'publish',data:{title:'New live title',slug:'example',items:[]}},'editor');assert.equal(published.status,200,await published.clone().text());
  const versions=await (await call('GET','?module=pages&id='+doc.id+'&operation=versions')).json();assert.ok(versions.docs.length>=2);
  await assert.rejects(p.restoreVersion({collection:'pages',id:String(versions.docs[0].id),user:{...users.editor,collection:'users'},overrideAccess:false} as never));
  current=await read();
  const other=await call('POST','',{module:'pages',mode:'publish',data:{title:'Other page',slug:'other',items:[]}});const otherDoc=(await other.json()).doc;
  const otherVersions=await (await call('GET','?module=pages&id='+otherDoc.id+'&operation=versions')).json();
  assert.equal((await call('POST','',{module:'pages',id:String(doc.id),mode:'restore',versionID:String(otherVersions.docs[0].id),updatedAt:current.updatedAt})).status,400);
  await assert.rejects(p.delete({collection:'pages',id:doc.id,user:{...users.editor,collection:'users'},overrideAccess:false} as never));
  const writes=await Promise.all(['A','B'].map(title=>call('POST','',{module:'pages',id:String(doc.id),updatedAt:current.updatedAt,mode:'publish',data:{title,slug:'example',items:[]}})));
  assert.equal(writes.filter(r=>r.status===200).length,1,'Concurrent saves must not silently overwrite each other');
  assert.ok(writes.some(r=>[409,500].includes(r.status)));
 }finally{await p.destroy();}
});
