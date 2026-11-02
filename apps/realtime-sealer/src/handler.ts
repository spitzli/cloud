import {createRealtime,rawSql} from '@neon/realtime/server';

export const topics=['workspace','operator-customers'] as const;
export type Topic=typeof topics[number];

/** Private SDK adapter. Core authorizes the caller; this process never accepts SQL or user parameters. */
export function createHandler(options:{secret:string;db:string}) {
  const realtime=createRealtime(options);
  const response=(value:unknown,status=200)=>Response.json(value,{status,headers:{'cache-control':'no-store'}});
  return async(request:Request):Promise<Response>=>{
    const url=new URL(request.url);
    if(request.method!=='POST'||url.pathname!=='/seal'||url.search)return response({error:'Not found'},404);
    if(!request.headers.get('content-type')?.match(/^application\/json(?:\s*;.*)?$/i))return response({error:'JSON required'},415);
    if(Number(request.headers.get('content-length'))>2048)return response({error:'Request too large'},413);
    let body:unknown;
    try {
      const text=await request.text();
      if(new TextEncoder().encode(text).length>2048)return response({error:'Request too large'},413);
      body=JSON.parse(text);
    } catch {return response({error:'Invalid request'},400)}
    if(!body||typeof body!=='object'||Array.isArray(body)||Object.keys(body).length!==1||!('topic' in body)||!topics.includes(body.topic as Topic))return response({error:'Invalid request'},400);
    try {
      return response(await realtime.seal({query:rawSql<{topic:Topic;revision:string}>('SELECT topic, revision FROM webdock_live.change_revision WHERE topic = $1',[body.topic as Topic])}));
    } catch {return response({error:'Realtime capability unavailable'},503)}
  };
}
