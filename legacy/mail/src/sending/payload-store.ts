import {open,unlink,statfs,link} from 'node:fs/promises';
import {constants} from 'node:fs';
import {isAbsolute,join} from 'node:path';
import {randomUUID} from 'node:crypto';
import {sealSecret,openSecret} from '@webdock/mail-core/secrets';
const LIMIT=25*1024*1024,SEALED_LIMIT=48*1024*1024;
/** The directory must be a dedicated private retained volume, owned by the Mail service. */
export class PrivatePayloadStore{
 private directory:string;
 private key:string;
 private initialized?:Promise<void>;
 constructor(directory:string,key:string){
  if(!isAbsolute(directory))throw Error('Private Mail storage requires an absolute path.');
  sealSecret(null,'payload-key-check',key);
  this.directory=directory;this.key=key;
 }
 async ready(){await this.verifyKey();}
 private verifyKey(){return this.initialized??=this.initialize();}
 private async initialize(){
  const marker=join(this.directory,'.mail-key-check');
  let file;
  try{file=await open(marker,constants.O_RDONLY|constants.O_NOFOLLOW);}
  catch(error){
   if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;
   const temporary=join(this.directory,'.mail-key-check-'+randomUUID());
   const created=await open(temporary,constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|constants.O_NOFOLLOW,0o600);
   try{await created.writeFile(sealSecret('webdock-private-payload-v1','payload-store',this.key));await created.sync();}finally{await created.close();}
   try{await link(temporary,marker);}catch(error){if((error as NodeJS.ErrnoException).code!=='EEXIST')throw error;}finally{await unlink(temporary).catch(()=>{});}
   const directory=await open(this.directory,constants.O_RDONLY|constants.O_DIRECTORY|constants.O_NOFOLLOW);try{await directory.sync();}finally{await directory.close();}
   file=await open(marker,constants.O_RDONLY|constants.O_NOFOLLOW);
  }
  try{const info=await file.stat();if(!info.isFile() || info.size>4096 || info.mode & 0o077)throw Error('Invalid Mail key marker.');const value=openSecret(await file.readFile('utf8'),'payload-store',this.key);if(value!=='webdock-private-payload-v1')throw Error('Mail payload key does not match its volume.');}
  finally{await file.close();}
 }
 private path(ref:string){if(!/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(ref))throw Error('Invalid Mail payload reference.');return join(this.directory,ref);}
 private purpose(customerID:string,ref:string){if(!/^[1-9][0-9]{0,18}$/.test(customerID))throw Error('Invalid Mail customer.');return `payload:${customerID}:${ref}`;}
 async put(customerID:string,body:Buffer,reference?:string){
  await this.verifyKey();
  if(!Buffer.isBuffer(body) || !body.length || body.length>LIMIT)throw Error('Mail payload exceeds its size limit.');
  const ref=reference??randomUUID(),path=this.path(ref),sealed=sealSecret(body.toString('base64'),this.purpose(customerID,ref),this.key);
  const space=await statfs(this.directory,{bigint:true});
  if(space.bavail*space.bsize<BigInt(Buffer.byteLength(sealed))*2n+104857600n)throw Error('Private Mail storage has insufficient free space.');
  const file=await open(path,constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|constants.O_NOFOLLOW,0o600);
  try{await file.writeFile(sealed);await file.sync();}
  catch(error){await unlink(path).catch(()=>{});throw error;}
  finally{await file.close();}
  const directory=await open(this.directory,constants.O_RDONLY|constants.O_DIRECTORY|constants.O_NOFOLLOW);
  try{await directory.sync();}finally{await directory.close();}
  return {ref,bytes:body.length};
 }
 async get(customerID:string,ref:string){
  await this.verifyKey();
  const path=this.path(ref),purpose=this.purpose(customerID,ref);
  const file=await open(path,constants.O_RDONLY|constants.O_NOFOLLOW);
  try{
   const stat=await file.stat();if(!stat.isFile() || stat.size>SEALED_LIMIT || stat.mode & 0o077)throw Error('Invalid private Mail payload.');
   // Bound the actual read even if the file changes after stat.
   const chunks:Buffer[]=[];let size=0;
   for await(const part of file.createReadStream({autoClose:false})){size+=part.length;if(size>SEALED_LIMIT)throw Error('Invalid private Mail payload.');chunks.push(part);}
   const encoded=openSecret<unknown>(Buffer.concat(chunks).toString('utf8'),purpose,this.key);
   if(typeof encoded!=='string')throw Error('Invalid private Mail payload.');
   const value=Buffer.from(encoded,'base64');if(!value.length || value.length>LIMIT || value.toString('base64')!==encoded)throw Error('Invalid private Mail payload.');return value;
  }finally{await file.close();}
 }
 /** Internal cleanup only; never expose an arbitrary reference deletion endpoint. */
 async remove(ref:string){await this.verifyKey();try{await unlink(this.path(ref));}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;}const directory=await open(this.directory,constants.O_RDONLY|constants.O_DIRECTORY|constants.O_NOFOLLOW);try{await directory.sync();}finally{await directory.close();}}
}
