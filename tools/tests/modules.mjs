import {readdirSync} from 'node:fs';
import {join} from 'node:path';
import {spawnSync} from 'node:child_process';
const walk=directory=>readdirSync(directory,{withFileTypes:true}).flatMap(entry=>entry.name==='node_modules'?[]:entry.isDirectory()?walk(join(directory,entry.name)):/\.test\.tsx?$/.test(entry.name)?[join(directory,entry.name)]:[]);
const sdk=spawnSync(process.execPath,['run','--filter','@webdock/plugin-sdk','build'],{stdio:'inherit'});
if(sdk.status!==0)process.exit(sdk.status??1);
const files=['packages/module-sdk','packages/plugin-sdk/tests','packages/core','packages/auth-better-auth','packages/auth-clerk','packages/auth-configuration','packages/platform-db','packages/hosting-provider-contracts','packages/console-kit','modules','adapters','integrations'].flatMap(walk).sort();
const result=spawnSync(process.execPath,['test','--preload','./legacy/control-plane/tests/helpers/no-network.ts',...files.map(file=>'./'+file)],{stdio:'inherit'});
process.exitCode=result.status??1;
