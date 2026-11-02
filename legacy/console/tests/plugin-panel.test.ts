import test from 'node:test';
import assert from 'node:assert/strict';
import {pluginPanelURL} from '../src/lib/plugin-panel';
test('installed extension panels stay on their configured HTTPS origin',()=>{
 assert.equal(pluginPanelURL('https://notes.example','/panel'), 'https://notes.example/panel');
 for(const path of ['https://attacker.example','//attacker.example','/\\attacker.example']) assert.equal(pluginPanelURL('https://notes.example',path),null);
 assert.equal(pluginPanelURL('http://notes.example','/panel'),null);
 assert.equal(pluginPanelURL(undefined,'/panel'),null);
});
test('HTTP plugin panels require explicit development loopback permission',()=>{
 const dev={allowLocalHTTP:true,environment:'development'};
 assert.equal(pluginPanelURL('http://localhost:3446','/ui/panel',dev),'http://localhost:3446/ui/panel');
 for(const origin of ['http://localhost.evil.test','http://192.168.1.4','http://example.test'])assert.equal(pluginPanelURL(origin,'/ui/panel',dev),null);
 assert.equal(pluginPanelURL('http://localhost:3446','/ui/panel',{...dev,environment:'production'}),null);
 assert.equal(pluginPanelURL('http://localhost:3446','/ui/panel'),null);
});
