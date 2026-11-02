import assert from 'node:assert/strict';
import {createHash,createPublicKey,verify} from 'node:crypto';

/** Fixture-only DKIM1 relaxed/RSA verification; production signing belongs to Stalwart. */
export function verifyFixtureDKIM(raw:Buffer,keys:{selector:string;publicKey:string}[],domain:string){
 const text=raw.toString('latin1'),split=text.indexOf('\r\n\r\n');assert.ok(split>0);
 const headers:{name:string;value:string}[]=[];
 for(const line of text.slice(0,split).split('\r\n')){
  if(/^[ \t]/.test(line)){assert.ok(headers.length);headers[headers.length-1].value+='\r\n'+line;}
  else{const colon=line.indexOf(':');assert.ok(colon>0);headers.push({name:line.slice(0,colon).toLowerCase(),value:line.slice(colon+1)});}
 }
 const signature=headers.find(header=>header.name==='dkim-signature' && /a=rsa-sha256/.test(header.value));assert.ok(signature);
 const tags=Object.fromEntries(signature.value.replace(/\r\n/g,'').split(';').map(part=>{const at=part.indexOf('=');return [part.slice(0,at).trim(),part.slice(at+1).trim()];}));
 assert.equal(tags.d,domain);assert.equal(tags.c,'relaxed/relaxed');assert.ok(tags.s.startsWith('webdock-'));
 let body=text.slice(split+4).split('\r\n').map(line=>line.replace(/[ \t]+/g,' ').replace(/[ \t]+$/,'')).join('\r\n').replace(/(?:\r\n)*$/,'');body+='\r\n';
 assert.equal(createHash('sha256').update(Buffer.from(body,'latin1')).digest('base64'),tags.bh.replace(/\s/g,''));
 const canonical=(value:string)=>value.replace(/\r\n[ \t]+/g,' ').replace(/[ \t]+/g,' ').trim();
 const remaining=[...headers];let signed='';
 for(const name of tags.h.toLowerCase().split(':').map(value=>value.trim())){
  const index=remaining.findLastIndex(header=>header.name===name);if(index<0)continue;
  const header=remaining.splice(index,1)[0];signed+=name+':'+canonical(header.value)+'\r\n';
 }
 signed+='dkim-signature:'+canonical(signature.value.replace(/\bb\s*=[^;]*/i,'b='));
 const key=keys.find(key=>key.selector===tags.s);assert.ok(key);
 assert.ok(verify('RSA-SHA256',Buffer.from(signed,'latin1'),createPublicKey({key:Buffer.from(key.publicKey.replace(/\s/g,''),'base64'),format:'der',type:'spki'}),Buffer.from(tags.b.replace(/\s/g,''),'base64')),'DKIM signature must verify');
 return tags.s;
}
