import {timingSafeEqual} from 'node:crypto';
import {parseCanvasDocument} from './model.ts';
import {CanvasError,canvasID,canvasRevision,canvasActor,type CanvasCommand,type CanvasStore} from './store.ts';
export const CANVAS_BODY_LIMIT=512*1024+2048;
export type CanvasRole='reader'|'editor'|'admin'|'operator';
const object=(value:unknown):value is Record<string,unknown>=>!!value&&typeof value==='object'&&!Array.isArray(value);
export function authorizeCanvas(request:Request,write=false,secret=process.env.SPITZLI_STUDIO_BRIDGE_SECRET){
 const supplied=request.headers.get('authorization')||'',expected='Bearer '+(secret||'');
 if(!secret||secret.length<32||Buffer.byteLength(supplied)!==Buffer.byteLength(expected)||!timingSafeEqual(Buffer.from(supplied),Buffer.from(expected)))throw new CanvasError('Unauthorized',401);
 const role=request.headers.get('x-webdock-role');if(!role||!['reader','editor','admin','operator'].includes(role)||(write&&role==='reader'))throw new CanvasError('Keine Berechtigung für diese Seitenaktion.',403);
 return {role:role as CanvasRole,actor:canvasActor(request.headers.get('x-webdock-actor'))};
}
export function parseCanvasCommand(raw:unknown):CanvasCommand{
 if(!object(raw)||typeof raw.action!=='string')throw new CanvasError('Ungültige Seitenaktion.');
 const fields:Record<string,string[]>={create:['action','document'],save:['action','id','expectedRevision','document'],publish:['action','id','expectedRevision'],unpublish:['action','id','expectedRevision'],homepage:['action','id','expectedRevision']};
 if(!Object.hasOwn(fields,raw.action)||Object.keys(raw).some(k=>!fields[raw.action as string].includes(k)))throw new CanvasError('Ungültige Seitenaktion.');
 const document=()=>{try{return parseCanvasDocument(raw.document)}catch{throw new CanvasError('Ungültiger Canvas-Entwurf. Bitte Felder und Grenzen prüfen.')}};
 switch(raw.action){case'create':return {action:'create',document:document()};case'save':return {action:'save',id:canvasID(raw.id),expectedRevision:canvasRevision(raw.expectedRevision),document:document()};case'publish':case'unpublish':return {action:raw.action,id:canvasID(raw.id),expectedRevision:canvasRevision(raw.expectedRevision)};case'homepage':return {action:'homepage',id:raw.id===null?null:canvasID(raw.id),...(raw.id!==null||raw.expectedRevision!==undefined?{expectedRevision:canvasRevision(raw.expectedRevision)}:{})};default:throw new CanvasError('Ungültige Seitenaktion.');}
}
export async function readCanvasJSON(request:Request){
 if(!request.headers.get('content-type')?.toLowerCase().startsWith('application/json'))throw new CanvasError('JSON erwartet.',415);
 if(Number(request.headers.get('content-length'))>CANVAS_BODY_LIMIT)throw new CanvasError('Der Canvas-Entwurf ist zu groß.',413);
 const reader=request.body?.getReader();if(!reader)throw new CanvasError('Leere Anfrage.');const chunks:Uint8Array[]=[];let total=0;
 try{for(;;){const {done,value}=await reader.read();if(done)break;total+=value.byteLength;if(total>CANVAS_BODY_LIMIT){await reader.cancel();throw new CanvasError('Der Canvas-Entwurf ist zu groß.',413);}chunks.push(value);}}finally{reader.releaseLock();}
 try{return JSON.parse(Buffer.concat(chunks).toString('utf8'))}catch{throw new CanvasError('Ungültiges JSON.');}
}
export type CanvasService=Pick<CanvasStore,'list'|'get'|'execute'>;
export async function handleCanvasRequest(request:Request,options:{store:()=>CanvasService;secret?:string}):Promise<Response>{
 const json=(data:unknown,status=200)=>Response.json(data,{status,headers:{'Cache-Control':'no-store'}});
 try{
  const write=request.method!=='GET';const access=authorizeCanvas(request,write,options.secret);
  if(request.method==='GET'){const p=new URL(request.url).searchParams;if([...p.keys()].some(k=>k!=='id')||p.getAll('id').length>1)throw new CanvasError('Ungültige Parameter.');if(p.has('id')){const id=p.get('id')!;if(!/^[1-9]\d{0,9}$/.test(id))throw new CanvasError('Ungültige Seiten-ID.');return json({page:await options.store().get(canvasID(Number(id)))});}return json(await options.store().list());}
  if(request.method!=='POST')return json({error:'Methode nicht erlaubt.'},405);
  if(new URL(request.url).search)throw new CanvasError('Ungültige Parameter.');const command=parseCanvasCommand(await readCanvasJSON(request));
  if(command.action==='homepage'&&!['admin','operator'].includes(access.role))throw new CanvasError('Nur Admins können die Startseite zuordnen.',403);
  return json(await options.store().execute(command,access.actor));
 }catch(error){return json({error:error instanceof CanvasError?error.message:'Der Seiteneditor ist derzeit nicht verfügbar.'},error instanceof CanvasError?error.status:503);}
}
