// Actual installed Next production runtime, exact public link component, no CMS/provider/database setup.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, copyFile, rm, readFile } from 'node:fs/promises';
import { dirname,join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { chromium } from 'playwright';
const repo=fileURLToPath(new URL('../../../',import.meta.url));
await mkdir(join(repo,'.runtime-test'),{recursive:true});
const fixture=await mkdtemp(join(repo,'.runtime-test/intent-link-'));
const next=join(dirname(fileURLToPath(import.meta.resolve('next/package.json'))),'dist/bin/next');
const source=fileURLToPath(new URL('../src/components/intent-link.tsx',import.meta.url));
let server,browser;
const env={PATH:process.env.PATH,NEXT_TELEMETRY_DISABLED:'1',NODE_ENV:'production'};
function launch(args){return spawn(process.execPath,['--bun',next,...args],{cwd:fixture,env,stdio:['ignore','pipe','pipe']});}
async function output(process){let text='';process.stdout.on('data',c=>{text+=c});process.stderr.on('data',c=>{text+=c});const [code]=await once(process,'exit');assert.equal(code,0,text);return text;}
try{
  await mkdir(join(fixture,'app/target'),{recursive:true});await mkdir(join(fixture,'components'));
  await copyFile(source,join(fixture,'components/intent-link.tsx'));
  assert.equal(await readFile(source,'utf8'),await readFile(join(fixture,'components/intent-link.tsx'),'utf8'));
  await writeFile(join(fixture,'package.json'),JSON.stringify({name:'owned-intent-fixture',private:true,type:'module',dependencies:{next:'16.3.8',react:'19.2.8','react-dom':'19.2.8'}}));
  await writeFile(join(fixture,'next.config.mjs'),'export default '+JSON.stringify({turbopack:{root:repo},outputFileTracingRoot:repo,experimental:{cpus:2}})+';');
  await writeFile(join(fixture,'app/layout.jsx'),'export default function Layout({children}){return <html lang="en"><body>{children}</body></html>}');
  await writeFile(join(fixture,'app/page.jsx'),'import {IntentLink} from "../components/intent-link";export default function Page(){return <main><h1>Owned intent fixture</h1><IntentLink href="/target">Open target</IntentLink></main>}');
  await writeFile(join(fixture,'app/target/page.jsx'),'export default function Target(){return <main><h1>Prefetched target</h1></main>}');
  await writeFile(join(fixture,'tsconfig.json'),JSON.stringify({compilerOptions:{target:'ES2022',lib:['dom','dom.iterable','esnext'],allowJs:true,skipLibCheck:true,strict:true,noEmit:true,module:'esnext',moduleResolution:'bundler',jsx:'react-jsx',isolatedModules:true,esModuleInterop:true},include:['app/**/*','components/**/*','.next/types/**/*.ts'],exclude:['node_modules']}));
  const built=await output(launch(['build',fixture]));assert.match(built,/Generating static pages/);
  const reserve=createServer();reserve.listen(0,'127.0.0.1');await once(reserve,'listening');const port=reserve.address().port;await new Promise(r=>reserve.close(r));const origin=`http://127.0.0.1:${port}`;
  server=launch(['start','--hostname','127.0.0.1','--port',String(port),fixture]);let log='';server.stdout.on('data',x=>{log+=x});server.stderr.on('data',x=>{log+=x});
  await new Promise((resolve,reject)=>{const limit=setTimeout(()=>reject(new Error(`Owned Next startup timed out: ${log}`)),15000);const ready=x=>{if(String(x).includes('Ready')){clearTimeout(limit);resolve();}};server.stdout.on('data',ready);server.once('exit',()=>{clearTimeout(limit);reject(new Error(log));});});
  browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE?{executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE}:{})});
  for(const intent of ['hover','focus']){
    const context=await browser.newContext();const page=await context.newPage();const errors=[],requests=[];page.on('pageerror',e=>errors.push(e.message));page.on('request',r=>{if(new URL(r.url()).pathname==='/target')requests.push(r)});
    await page.goto(origin);const link=page.getByRole('link',{name:'Open target'});await link.waitFor();await page.waitForLoadState('networkidle');assert.equal(requests.length,0,'viewport visibility alone must not prefetch');
    const warm=page.waitForRequest(r=>new URL(r.url()).pathname==='/target'&&r.headers()['next-router-prefetch']==='1');if(intent==='hover')await link.hover();else await link.focus();await warm;assert.equal(page.url(),`${origin}/`,'prefetch does not navigate');
    await link.click();await page.getByRole('heading',{name:'Prefetched target'}).waitFor();assert.equal(page.url(),`${origin}/target`);assert.equal(requests.filter(r=>r.isNavigationRequest()).length,0,'Next Link makes a client transition');assert.deepEqual(errors,[]);await context.close();
  }
  console.log('Next production intent PASS: no viewport prefetch; real RSC prefetch on hover/focus; client navigation. Exact source, no provider/DB config.');
}finally{
  await browser?.close();if(server&&server.exitCode===null){server.kill('SIGTERM');await once(server,'exit');}await rm(fixture,{recursive:true,force:true});
}
