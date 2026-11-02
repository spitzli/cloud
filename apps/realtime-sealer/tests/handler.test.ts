import {test,expect} from 'bun:test';
import {createHandler} from '../src/handler';
const secret='nrt_live_1'+Buffer.alloc(32,7).toString('base64url');
const handler=createHandler({secret,db:'neondb'});
function request(body:unknown){return new Request('http://private/seal',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)})}
test('only fixed invalidation topics are sealed with bounded expiry and no caching',async()=>{
 for(const topic of ['workspace','operator-customers']){
  const response=await handler(request({topic}));expect(response.status).toBe(200);expect(response.headers.get('cache-control')).toBe('no-store');const value=await response.json();expect(typeof value.capability).toBe('string');expect(value.capability).not.toContain('change_revision');expect(value.capability).not.toContain(secret);expect(value.expiresAt).toBeGreaterThan(Date.now());expect(value.expiresAt).toBeLessThanOrEqual(Date.now()+61000);expect(typeof value.queryFingerprint).toBe('string');expect(Object.keys(value).sort()).toEqual(['capability','expiresAt','queryFingerprint']);
 }
});
test('raw SQL, actor input, extra keys, query strings and methods cannot widen the sealer',async()=>{
 for(const body of [{topic:'private-users'}, {topic:'workspace',sql:'SELECT password FROM auth'}, {topic:'workspace',userID:'other'},null,[]])expect((await handler(request(body))).status).toBe(400);
 expect((await handler(new Request('http://private/seal'))).status).toBe(404);
 expect((await handler(new Request('http://private/seal?topic=workspace',{method:'POST',body:'{}'}))).status).toBe(404);
 expect((await handler(request({topic:'x'.repeat(3000)}))).status).toBe(413);
});
