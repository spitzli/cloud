import test from 'node:test';
import assert from 'node:assert/strict';
import {commandSchema} from './index';
const spec={template:'custom',image:'registry.example/app@sha256:'+'a'.repeat(64),args:[],port:8080,healthPath:'/',cpuMillicores:100,memoryBytes:16777216,ephemeralBytes:1048576,replicas:1,volumeBytes:0};
const create={action:'apps.create',projectID:'1',name:'App',spec,subscriptionRevision:1,idempotencyKey:'public-address-create'};
test('public address requests choose a name without supplying ownership or routing authority',()=>{
 assert.ok(commandSchema.safeParse(create).success);
 for(const publicAddress of [{},{label:'lunares'}])assert.ok(commandSchema.safeParse({...create,publicAddress}).success);
 for(const publicAddress of [true,null,{hostname:'other.spitzli.site'},{label:'../escape'},{label:'a.b'},{label:'*'},{label:'x',customerID:'2'}])assert.equal(commandSchema.safeParse({...create,publicAddress}).success,false);
 assert.ok(commandSchema.safeParse({action:'addresses.list',customerID:'1'}).success);
 const policy={action:'clusters.publicSites.configure',clusterID:'1',domain:'spitzli.site',enabled:true,policyRevision:0,tenantDailyLimit:1,globalDailyLimit:2,trustedCustomerIDs:['1']};
 assert.ok(commandSchema.safeParse(policy).success);
 assert.equal(commandSchema.safeParse({...policy,globalDailyLimit:0}).success,false);
});
