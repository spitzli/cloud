import {useEffect} from 'react';
import {useQueryClient} from '@tanstack/react-query';
import {createRealtimeClient} from '@neon/realtime/client';
import {z} from 'zod';
import {useApi,useSession} from './context';
import {startLiveInvalidation} from './realtime';
const capabilitySchema=z.object({capability:z.string().min(1).max(12000),queryFingerprint:z.string().min(1).max(256),expiresAt:z.number().int().positive()}).strict();
export function LiveUpdates(){
 const api=useApi(),session=useSession(),cache=useQueryClient();
 const url=api.config.realtimeUrl;
 const allowed=!session.user.preview&&!session.user.acting;
 const topic=session.user.role==='operator'?'operator-customers':'workspace';
 useEffect(()=>{
  if(!url||!allowed)return;
  const abort=new AbortController();
  const client=createRealtimeClient({url});
  void startLiveInvalidation({client,signal:abort.signal,
   capability:()=>api.request('/api/console/realtime/capability',capabilitySchema,{method:'POST',body:JSON.stringify({topic}),signal:abort.signal}),
   changed:()=>{void cache.invalidateQueries({predicate:query=>query.queryKey[0]!=='runtime-config'})},
  });
  return()=>{abort.abort()};
 },[url,allowed,topic,api,cache,session.user.id,session.session.id]);
 return null;
}
