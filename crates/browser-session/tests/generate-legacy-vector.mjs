// Deterministic compatibility fixture only: never use this entropy in an application.
import { webcrypto } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { seal, defaults } from 'iron-webcrypto';
const now=Date.parse('2026-10-19T12:00:00Z');
const saved=Date.now;Date.now=()=>now;
try {
  const password='fixture-retained-cookie-secret-long-enough';
  const value={kind:'session',accessToken:'fixture-opaque-access',sub:'123',exp:now/1000+3600,idToken:'fixture-id-token'};
  const crypto={subtle:webcrypto.subtle,getRandomValues:buffer=>buffer.fill(7)};
  const token=await seal(crypto,value,{id:'1',secret:password},{...defaults,ttl:3600000});
  writeFileSync(new URL('./legacy-vector.json',import.meta.url),JSON.stringify({now:now/1000,password,value,token:token+'~2'},null,2)+'\n');
} finally {Date.now=saved;}
