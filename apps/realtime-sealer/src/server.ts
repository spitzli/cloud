import {chmod} from 'node:fs/promises';
import {prepareSocket,removeSocket} from './socket';
import {isAbsolute} from 'node:path';
import {createHandler} from './handler';

if(import.meta.main){
  process.umask(0o077);
  const socket=process.env.SPITZLI_REALTIME_SOCKET;
  const secret=process.env.NEON_REALTIME_SECRET;
  const db=process.env.SPITZLI_REALTIME_DATABASE;
  if(!socket||!isAbsolute(socket)||!secret||!db)throw Error('Explicit private realtime configuration required');
  await prepareSocket(socket);
  const server=Bun.serve({unix:socket,maxRequestBodySize:2048,fetch:createHandler({secret,db})});await chmod(socket,0o600);
  const stop=async()=>{server.stop(true);await removeSocket(socket);process.exit(0)};process.once('SIGTERM',stop);process.once('SIGINT',stop);
  console.log('Private realtime sealer ready');
}
