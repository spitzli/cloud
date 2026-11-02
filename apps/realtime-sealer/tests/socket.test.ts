import {test,expect} from 'bun:test';
import {mkdtemp,chmod,lstat,rm,symlink,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {prepareSocket,removeSocket} from '../src/socket';

test('private socket lifecycle refuses active sockets and unrelated paths, reclaims owned stale sockets',async()=>{
 const dir=await mkdtemp(join(tmpdir(),'spitzli-realtime-'));await chmod(dir,0o700);const path=join(dir,'sdk.sock');
 try{
  await prepareSocket(path);
  const server=Bun.serve({unix:path,fetch:()=>new Response('ok')});await chmod(path,0o600);
  await expect(prepareSocket(path)).rejects.toThrow();expect((await lstat(path)).isSocket()).toBe(true);
  server.stop(true);await removeSocket(path);
  await writeFile(path,'keep');await expect(prepareSocket(path)).rejects.toThrow();await expect(removeSocket(path)).rejects.toThrow();await rm(path);
  await symlink(join(dir,'missing'),path);await expect(prepareSocket(path)).rejects.toThrow();await rm(path);
  const child=Bun.spawn([process.execPath,'-e',`Bun.serve({unix:process.argv[1],fetch:()=>new Response('ok')});console.log('ready');await new Promise(()=>{});`,path],{stdout:'pipe',stderr:'pipe'});
  await child.stdout.getReader().read();child.kill('SIGKILL');await child.exited;
  // Some Bun versions remove the path when stopping; SIGKILL leaves a real stale socket.
  await prepareSocket(path);expect(await Bun.file(path).exists()).toBe(false);
  await chmod(dir,0o755);await expect(prepareSocket(path)).rejects.toThrow();
 }finally{await rm(dir,{recursive:true,force:true})}
});
