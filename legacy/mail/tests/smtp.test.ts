import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:tls';
import {once} from 'node:events';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {X509Certificate,createHash} from 'node:crypto';
import {submitSMTP,SMTPFailure} from '../src/sending/smtp.ts';

async function fixture(mode:'accept'|'drop'|'auth-denied') {
  const dir=mkdtempSync(join(tmpdir(),'webdock-smtp-'));
  execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=localhost','-keyout',join(dir,'key.pem'),'-out',join(dir,'cert.pem')],{stdio:'ignore'});
  const key=readFileSync(join(dir,'key.pem')),cert=readFileSync(join(dir,'cert.pem'));rmSync(dir,{recursive:true});
  const certificateSha256=createHash('sha256').update(new X509Certificate(cert).raw).digest('hex');
  let connections=0,auth=0,accepted=0;const lines:string[]=[];
  const server=createServer({key,cert},socket=>{
    connections++;socket.setEncoding('utf8');socket.write('220 fixture ESMTP\r\n');let buffer='',data=false;
    socket.on('data',chunk=>{buffer+=chunk;for(;;){const end=buffer.indexOf('\r\n');if(end<0)break;const line=buffer.slice(0,end);buffer=buffer.slice(end+2);
      if(data){if(line==='.'){data=false;if(mode==='drop'){socket.destroy();return;}accepted++;socket.write('250 2.0.0 Accepted\r\n');}else lines.push(line);continue;}
      if(line.startsWith('EHLO'))socket.write('250-fixture\r\n250-AUTH PLAIN\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n');
      else if(line.startsWith('AUTH')){auth++;socket.write(mode==='auth-denied'?'535 5.7.8 Denied\r\n':'235 2.7.0 OK\r\n');}
      else if(line==='DATA'){data=true;socket.write('354 Continue\r\n');}
      else if(line==='QUIT')socket.end('221 Bye\r\n');
      else socket.write('250 OK\r\n');
    }});socket.on('error',()=>{});
  });server.listen(0,'127.0.0.1');await once(server,'listening');const address=server.address();assert.ok(address && typeof address!=='string');
  return {options:{host:'127.0.0.1',port:address.port,certificateSha256,username:'fixture',password:'fixture-password',allowLoopback:true},stats:()=>({connections,auth,accepted,lines}),close:()=>new Promise<void>(r=>server.close(()=>r()))};
}
const message={from:'sender@example.test',to:['recipient@example.test'],raw:Buffer.from('From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: Fixture\r\n\r\n.line starting with a dot\r\n')};

test('SMTP verifies the instance certificate before credentials and waits for final DATA acceptance',async()=>{
 const f=await fixture('accept');try{await submitSMTP(f.options,message);assert.equal(f.stats().accepted,1);assert.ok(f.stats().lines.includes('..line starting with a dot'));}finally{await f.close();}
});
test('SMTP never discloses credentials to a mismatched instance certificate',async()=>{
 const f=await fixture('accept');try{await assert.rejects(submitSMTP({...f.options,certificateSha256:'0'.repeat(64)},message),e=>e instanceof SMTPFailure && !e.uncertain);assert.equal(f.stats().auth,0);}finally{await f.close();}
});
test('a lost final DATA reply stays uncertain and is never automatically retried',async()=>{
 const f=await fixture('drop');try{await assert.rejects(submitSMTP(f.options,message),e=>e instanceof SMTPFailure && e.uncertain);assert.equal(f.stats().connections,1);}finally{await f.close();}
});
test('an explicit authentication rejection is known not accepted',async()=>{
 const f=await fixture('auth-denied');try{await assert.rejects(submitSMTP(f.options,message),e=>e instanceof SMTPFailure && !e.uncertain && e.responseCode===535);assert.equal(f.stats().accepted,0);}finally{await f.close();}
});
