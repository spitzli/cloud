import test from 'node:test';
import assert from 'node:assert/strict';
import {pluginLaunchLinks} from '../src/lib/plugin-launch-links';

const link = {appID:'123',label:'Customer registry',appURL:'https://registry.example.test',consolePath:'/hosting/apps/123'};
test('host-provided app links preserve the dedicated origin and exact hosting resource',()=>{
 assert.deepEqual(pluginLaunchLinks([link]),[link]);
 for (const patch of [{appURL:'javascript:alert(1)'},{appURL:'https://user:password@registry.example.test'},{appURL:'https://registry.example.test/path'},{appURL:'http://registry.example.test'},{consolePath:'//evil.example'},{consolePath:'/hosting/apps/456'},{appID:'../123'}]) {
  assert.deepEqual(pluginLaunchLinks([{...link,...patch}]),[]);
 }
 assert.deepEqual(pluginLaunchLinks(undefined),[]);
 assert.deepEqual(pluginLaunchLinks([null,{},link]),[link]);
});
