import type {RealtimeClient,SealedLiveQuery,RawLiveQuerySubscription} from '@neon/realtime/client';

/** A subscription can refresh data, but every data read still goes through the authorized API. */
export async function startLiveInvalidation(options:{client:Pick<RealtimeClient,'subscribe'|'close'>;capability:()=>Promise<SealedLiveQuery<unknown>>;changed:()=>void;signal:AbortSignal}){
 let subscription:RawLiveQuerySubscription<unknown>|undefined;
 let timer:ReturnType<typeof setTimeout>|undefined;
 let closed=false,retry=1000;
 const stop=()=>{if(closed)return;closed=true;clearTimeout(timer);subscription?.unsubscribe();options.client.close()};
 const schedule=(delay:number)=>{clearTimeout(timer);if(!closed)timer=setTimeout(()=>{void renew()},delay)};
 const failed=(error:unknown)=>{
  if(closed)return;
  const status=error&&typeof error==='object'&&'status' in error?error.status:undefined;
  if(typeof status==='number'&&status>=400&&status<500){stop();options.changed();return}
  schedule(retry);retry=Math.min(retry*2,15000);
 };
 options.signal.addEventListener('abort',stop,{once:true});
 if(options.signal.aborted){stop();return}
 const renew=async()=>{
  try{
   const sealed=await options.capability();if(closed)return;
   if(subscription){
    // A transport ACK must not block subsequent authorization refreshes during an outage.
    void subscription.renew(sealed).catch(failed);
   }else {subscription=options.client.subscribe(sealed,{materialize:false});subscription.onUpdate(options.changed)}
   retry=1000;schedule(Math.max(1000,sealed.expiresAt-Date.now()-15000));
  }catch(error){failed(error)}
 };
 await renew();
}
