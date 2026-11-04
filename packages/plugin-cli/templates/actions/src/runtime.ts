import {createServer} from 'node:https';
import {createPublicKey, timingSafeEqual} from 'node:crypto';
import {createPluginContext, verifyInvocation} from '@spitzli/plugin-sdk/server';

type RuntimeOptions = {
 pluginID:string;version:string;generation:number;hostURL:string;
 runtimeToken:string;publicKey:string;cert:Buffer;key:Buffer;
};

function echo(input:unknown, _context:ReturnType<typeof createPluginContext>) {
 if(!input || typeof input!=='object' || !('message' in input) || typeof input.message!=='string' || input.message.length>1000) throw Error('invalid_input');
 return {message:input.message};
}

export function createActionServer(options:RuntimeOptions) {
 const {pluginID,version,generation,hostURL,runtimeToken,publicKey}=options;
 if(!/^[a-z][a-z0-9-]{0,63}$/.test(pluginID) || !version || version.length>128 || !Number.isSafeInteger(generation) || generation<1 || runtimeToken.length<32) throw Error('Invalid runtime identity');
 if(createPublicKey(publicKey).asymmetricKeyType!=='ed25519') throw Error('Expected Ed25519 host key');
 // Validate the configured host origin at startup; never use a request-supplied host URL.
 createPluginContext({hostURL,token:()=>'',runtimeCredential:runtimeToken});
 const expected=Buffer.from(`Bearer ${runtimeToken}`);
 const server=createServer({cert:options.cert,key:options.key,minVersion:'TLSv1.2'},async(req,res)=>{
  res.setHeader('Content-Type','application/json');
  res.setHeader('Cache-Control','no-store');
  const reply=(status:number,body:unknown)=>{res.writeHead(status);res.end(JSON.stringify(body));};
  const supplied=Buffer.from(req.headers.authorization??'');
  if(supplied.length!==expected.length || !timingSafeEqual(supplied,expected)) {reply(401,{error:'unauthorized'});return;}
  if(req.method==='GET' && req.url==='/healthz') {reply(200,{pluginID,version,generation,protocolVersion:1});return;}
  if(req.method!=='POST' || req.url!=='/actions/demo.echo') {reply(404,{error:'unknown_action'});return;}
  const invocation=req.headers['x-plugin-invocation'];
  try {
   if(typeof invocation!=='string') throw Error();
   const claims=verifyInvocation(invocation,{publicKey,pluginID,generation});
   if(claims.action!=='demo.echo') throw Error();
  } catch {reply(403,{error:'invalid_invocation'});return;}
  try {
   let size=0;
   const chunks:Buffer[]=[];
   for await(const chunk of req) {
    size+=chunk.length;
    if(size>32768) {reply(413,{error:'too_large'});return;}
    chunks.push(chunk);
   }
   const body:unknown=JSON.parse(Buffer.concat(chunks).toString('utf8'));
   if(!body || typeof body!=='object' || !('input' in body)) throw Error();
   const context=createPluginContext({hostURL,token:()=>invocation as string,runtimeCredential:runtimeToken});
   reply(200,echo(body.input,context));
  } catch {reply(400,{error:'invalid_input'});}
 });
 server.requestTimeout=10000;
 server.headersTimeout=5000;
 server.maxHeadersCount=32;
 return server;
}
