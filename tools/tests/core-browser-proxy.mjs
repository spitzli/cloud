/** Raw HTTP/WebSocket ingress for the owned disposable local fixture. */
import http from 'node:http';
import {readFileSync} from 'node:fs';
export function browserProxies(ingressSecret){
 return [3460,3461,3465].map(port=>{
  function options(request){
   const api=/^\/(api\/(auth|sso|mcp|mail|hosting|databases)(\/|$)|v1(\/|$)|\.well-known(\/|$))/.test(request.url);
   const upstream=api?3463:port===3465?3475:3470;
   const headers={...request.headers,host:api?'127.0.0.1:'+upstream:'localhost:'+port,'x-forwarded-host':'localhost:'+port,'x-forwarded-proto':'http'};
   delete headers['x-webdock-client-ip'];delete headers['x-webdock-ingress-token'];
   if(api){headers['x-forwarded-for']='127.0.0.1';headers['x-webdock-ingress-token']=ingressSecret;}
   return {hostname:'127.0.0.1',port:upstream,path:request.url,method:request.method,headers};
  }
  const server=http.createServer((request,response)=>{
   const upstream=http.request(options(request),incoming=>{response.writeHead(incoming.statusCode,incoming.rawHeaders);incoming.pipe(response);});
   upstream.on('error',()=>{if(!response.headersSent)response.writeHead(502);response.end('Local fixture upstream unavailable');});
   request.pipe(upstream);response.on('close',()=>upstream.destroy());
  });
  server.on('upgrade',(request,socket,head)=>{
   const upstream=http.request(options(request));
   upstream.on('upgrade',(response,remote,upstreamHead)=>{
    socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n`);
    for(let i=0;i<response.rawHeaders.length;i+=2)socket.write(response.rawHeaders[i]+': '+response.rawHeaders[i+1]+'\r\n');
    socket.write('\r\n');if(upstreamHead.length)socket.write(upstreamHead);if(head.length)remote.write(head);
    socket.pipe(remote).pipe(socket);remote.on('error',()=>socket.destroy());socket.on('error',()=>remote.destroy());
   });
   upstream.on('response',response=>{socket.end(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\nConnection: close\r\n\r\n`);});
   upstream.on('error',()=>socket.destroy());upstream.end();
  });
  server.listen(port,'127.0.0.1');return {stop(){server.close();server.closeAllConnections();}};
 });
}
if(process.argv[1]?.endsWith('core-browser-proxy.mjs')){
 const fixture=JSON.parse(readFileSync(process.argv[2],'utf8'));
 if(!/^webdock-browser-[a-f0-9]+$/.test(fixture.container))throw Error('Owned fixture required');
 browserProxies(fixture.env.SPITZLI_INGRESS_SECRET);console.log('Owned raw HTTP/WebSocket proxy ready');
}
