import {mkdir,lstat,unlink} from 'node:fs/promises';
import {isAbsolute,dirname} from 'node:path';
import {createConnection} from 'node:net';

async function ownedSocket(path:string){
 const stat=await lstat(path);
 if(!stat.isSocket()||stat.isSymbolicLink()||stat.uid!==process.getuid!()||stat.nlink!==1)throw Error('Owned realtime socket required');
 return stat;
}
export async function prepareSocket(path:string){
 if(!isAbsolute(path))throw Error('Absolute realtime socket required');
 const parent=dirname(path);await mkdir(parent,{recursive:true,mode:0o700});const dir=await lstat(parent);
 if(!dir.isDirectory()||dir.isSymbolicLink()||dir.uid!==process.getuid!()||(dir.mode&0o077)!==0)throw Error('Owned private realtime directory required');
 let original;
 try{original=await ownedSocket(path)}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return;throw error}
 await new Promise<void>((resolve,reject)=>{
  const client=createConnection({path});
  client.setTimeout(1000,()=>{client.destroy();reject(Error('Realtime socket probe timed out'))});
  client.once('connect',()=>{client.destroy();reject(Error('Realtime socket is active'))});
  client.once('error',(error:NodeJS.ErrnoException)=>{client.destroy();if(error.code==='ECONNREFUSED')resolve();else reject(Error('Realtime socket probe failed'))});
 });
 const current=await ownedSocket(path);
 if(current.ino!==original.ino||current.dev!==original.dev)throw Error('Realtime socket changed');
 await unlink(path);
}
export async function removeSocket(path:string){
 try{await ownedSocket(path);await unlink(path)}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error}
}
