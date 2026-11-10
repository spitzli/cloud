import {readFileSync} from 'node:fs';
import {createActionServer} from './runtime.js';

function required(name:string):string {
 const value=process.env[name];
 if(!value) throw Error(`Missing ${name}`);
 return value;
}

const server=createActionServer({
 pluginID:required('SPITZLI_PLUGIN_ID'),
 version:required('SPITZLI_PLUGIN_VERSION'),
 generation:Number(required('SPITZLI_PLUGIN_GENERATION')),
 hostURL:required('SPITZLI_PLUGIN_HOST_URL'),
 runtimeToken:required('SPITZLI_PLUGIN_RUNTIME_TOKEN'),
 publicKey:required('SPITZLI_PLUGIN_PUBLIC_KEY'),
 cert:readFileSync(required('SPITZLI_PLUGIN_TLS_CERT')),
 key:readFileSync(required('SPITZLI_PLUGIN_TLS_KEY')),
});
server.listen(3443,'0.0.0.0');
process.on('SIGTERM',()=>{
 server.close(()=>process.exit(0));
 setTimeout(()=>process.exit(1),10000).unref();
});
