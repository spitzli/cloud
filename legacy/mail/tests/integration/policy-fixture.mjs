// Disposable, isolated Docker fixture. Not a deployable policy service.
import {createServer} from 'node:http';
import {createServer as tcpServer} from 'node:net';
const received=[];
let captured, unavailable=false,accepted=false;
createServer(async(req,res)=>{
 res.setHeader('Content-Type','application/json');
 if(req.url==='/received'){res.end(JSON.stringify(received));return;}
 if(req.url==='/healthz'){res.end('{}');return;}
 if(req.url==='/captured'){res.end(JSON.stringify(captured??null));return;}
 if(req.url==='/accept'){unavailable=false;accepted=true;res.end('{}');return;}
 if(req.url==='/unavailable'){unavailable=true;res.end('{}');return;}
 if(req.headers.authorization!=='Bearer disposable-policy-token'){res.writeHead(401);res.end('{}');return;}
 const chunks=[];for await(const part of req)chunks.push(part);
 captured=JSON.parse(Buffer.concat(chunks).toString());
 if(unavailable){res.writeHead(503);res.end('{}');return;}
 if(accepted){res.end(JSON.stringify({action:'accept',modifications:[{type:'deleteHeader',index:1,name:'X-Webdock-Submission'}]}));return;}
 res.end(JSON.stringify({action:'reject',response:{status:550,enhanced_status:'5.7.1',message:'Webdock fixture policy denied'}}));
}).listen(8090,'0.0.0.0');

tcpServer(socket=>{
 socket.setEncoding('utf8');socket.write('220 isolated-fixture ESMTP\r\n');
 let buffer='',data=false,content='',envelope={from:'',to:[]};
 socket.on('data',chunk=>{
  buffer+=chunk;
  for(;;){const end=buffer.indexOf('\r\n');if(end<0)break;const line=buffer.slice(0,end);buffer=buffer.slice(end+2);
   if(data){if(line==='.') {received.push({...envelope,content});data=false;content='';socket.write('250 2.0.0 queued by isolated sink\r\n');}else content+=line+'\r\n';continue;}
   if(/^EHLO|^HELO/i.test(line))socket.write('250 isolated-fixture\r\n');
   else if(/^MAIL FROM:/i.test(line)){envelope={from:line.slice(10),to:[]};socket.write('250 2.1.0 OK\r\n');}
   else if(/^RCPT TO:/i.test(line)){envelope.to.push(line.slice(8));socket.write('250 2.1.5 OK\r\n');}
   else if(line==='DATA'){data=true;socket.write('354 End with dot\r\n');}
   else if(line==='QUIT'){socket.end('221 Bye\r\n');}
   else socket.write('250 OK\r\n');
  }
 });socket.on('error',()=>{});
}).listen(2526,'0.0.0.0');
