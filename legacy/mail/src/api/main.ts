import {mkdir,lstat} from 'node:fs/promises';
import {setTimeout as delay} from 'node:timers/promises';
import {createMailAPI} from './server.ts';
import {mailControlClient} from './control.ts';
import {PrivatePayloadStore} from '../sending/payload-store.ts';
import {processQueuedSend,cleanupQueuedPayloads} from '../sending/queued-worker.ts';
const required=(name:string)=>{const value=process.env[name];if(!value)throw Error(`Configure ${name}`);return value;};
if(process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED!=='true')throw Error('Native Mail sending is disabled.');
const directory=required('MAIL_PAYLOAD_DIRECTORY');await mkdir(directory,{recursive:true,mode:0o700});
const stat=await lstat(directory);if(!stat.isDirectory() || stat.isSymbolicLink() || stat.mode & 0o077)throw Error('Mail payload directory must be private.');
const store=new PrivatePayloadStore(directory,required('MAIL_ENCRYPTION_KEY'));await store.ready();
if(process.env.MAIL_SEND_WORKER_SECRET===process.env.MAIL_GATEWAY_SECRET)throw Error('Mail worker and browser control secrets must be separate.');
const control=mailControlClient(required('WEBDOCK_AUTH_ORIGIN'),required('MAIL_SEND_WORKER_SECRET'));
const server=createMailAPI(control,store,{webmailSecret:process.env.MAIL_GATEWAY_SECRET}),port=Number(process.env.PORT||3139);
if(!Number.isSafeInteger(port) || port<1024 || port>65535)throw Error('Invalid Mail API port.');
let stopping=false,lastCleanup=0;
for(const signal of ['SIGINT','SIGTERM'] as const)process.once(signal,()=>{stopping=true;server.close();setTimeout(()=>server.closeAllConnections(),30000).unref();});
server.listen(port,process.env.HOST||'127.0.0.1',()=>console.log('Webdock Mail API listening'));
while(!stopping){
 try{if(Date.now()-lastCleanup>30000){await cleanupQueuedPayloads(control,store);lastCleanup=Date.now();}const worked=await processQueuedSend(control,store);if(!worked)await delay(2000);}
 catch{console.error('Mail dispatch paused; existing handoffs will not be retried.');await delay(5000);}
}
