import test from 'node:test';
import assert from 'node:assert/strict';
import { POST } from '../src/app/api/locale/route';
import { date } from '../src/lib/presentation';
import { cmsRoleLabel } from '../src/lib/cms-modules';
import { uiLabel } from '../src/lib/ui-labels';

test('Studio locale endpoint accepts only its configured origin and writes a preference cookie', async () => {
  const origin = new URL(process.env.NEXT_PUBLIC_SERVER_URL || 'https://studio.webdock.dev').origin;
  const response = await POST(new Request(origin + '/api/locale', { method: 'POST', headers: { origin, 'content-type': 'application/json' }, body: JSON.stringify({preference:'de'}) }));
  assert.equal(response.status, 204);
  const cookie = response.headers.get('set-cookie') || '';
  assert.match(cookie, /webdock_locale=de/);
  assert.match(cookie, /HttpOnly/i);
  assert.doesNotMatch(cookie, /Domain=/i);
  const denied = await POST(new Request(origin + '/api/locale', { method: 'POST', headers: { origin:'https://untrusted.example', 'content-type':'application/json' }, body: JSON.stringify({preference:'en'}) }));
  assert.equal(denied.status,403);
  assert.equal(denied.headers.get('set-cookie'),null);
});
test('presentation helpers change labels and date display without changing role identifiers', () => {
  assert.equal(cmsRoleLabel('admin'),'Tenant admin');
  assert.equal(cmsRoleLabel('reader'),'Read-only');
  assert.equal(uiLabel('READY'),'Ready');
  assert.equal(uiLabel('active'),'Active');
  assert.equal(uiLabel('unknown-state'),'unknown-state');
  const timestamp='2026-10-05T12:00:00.000Z';
  assert.equal(date(timestamp,'en'),'5 Oct 2026');
  assert.equal(date(timestamp,'de'),'5. Okt. 2026');
});

test('locale rendering does not translate submitted values, URLs, IDs, or editor keys', async () => {
  const { readdir, readFile } = await import('node:fs/promises');
  const { join } = await import('node:path');
  const ts = await import('typescript');
  async function scan(directory: string): Promise<void> {
    for (const entry of await readdir(directory,{withFileTypes:true})) {
      const file=join(directory,entry.name);
      if(entry.isDirectory()) { await scan(file); continue; }
      if(!file.endsWith('.tsx')) continue;
      const source=ts.createSourceFile(file,await readFile(file,'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
      function visit(node: import('typescript').Node) {
        if(ts.isJsxAttribute(node)&&['value','defaultValue','name','type','href','action','key','id'].includes(node.name.getText(source))) {
          assert.doesNotMatch(node.initializer?.getText(source)||'',/i18n\.(?:t|n|p|locale)\b/,`${file}: ${node.name.getText(source)} must preserve data and component identity`);
        }
        ts.forEachChild(node,visit);
      }
      visit(source);
    }
  }
  await scan('src');
});

test('GNU gettext Studio catalog covers UI source literals and native error aliases', async () => {
  const { readdir, readFile, mkdtemp, rm, writeFile } = await import('node:fs/promises');
  const { join } = await import('node:path');
  const { tmpdir } = await import('node:os');
  const { spawnSync } = await import('node:child_process');
  const ts = await import('typescript');
  const messages=new Set<string>();
  async function scan(directory:string):Promise<void>{
    for(const entry of await readdir(directory,{withFileTypes:true})){
      const file=join(directory,entry.name);
      if(entry.isDirectory()){await scan(file);continue;}
      if(!/\.tsx?$/.test(file))continue;
      const source=ts.createSourceFile(file,await readFile(file,'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
      function visit(node:import('typescript').Node){
        if(ts.isCallExpression(node)&&/(?:^|\.)(?:t|msgid|n)$/.test(node.expression.getText(source))&&node.arguments[0]&&ts.isStringLiteral(node.arguments[0])&&node.arguments[0].text)messages.add(node.arguments[0].text);
        ts.forEachChild(node,visit);
      }
      visit(source);
    }
  }
  await scan('src');
  const aliases=JSON.parse(await readFile('../../packages/i18n/locales/legacy-studio.json','utf8')) as Record<string,string>;
  for(const message of Object.values(aliases))messages.add(message);
  const directory=await mkdtemp(join(tmpdir(),'studio-gettext-'));
  try{
    const mo=join(directory,'studio.mo'), ids=join(directory,'ids.json');
    await writeFile(ids,JSON.stringify([...messages]));
    const merged=join(directory,'studio.po');
    const catalogs=(await readdir('../../packages/i18n/locales/de')).filter(name=>name.endsWith('.po')).sort().map(name=>join('../../packages/i18n/locales/de',name));
    const combined=spawnSync('msgcat',['--use-first','-o',merged,...catalogs],{encoding:'utf8'});
    assert.equal(combined.status,0,combined.stderr);
    const compiled=spawnSync('msgfmt',['--check','--check-format','-o',mo,merged],{encoding:'utf8'});
    assert.equal(compiled.status,0,compiled.stderr);
    const checked=spawnSync('python3',['-c',`import gettext,json,sys
with open(sys.argv[1],'rb') as stream: catalog=gettext.GNUTranslations(stream)._catalog
missing=[key for key in json.load(open(sys.argv[2])) if not catalog.get(key) and not catalog.get((key,0))]
assert not missing, 'Missing Studio messages: '+repr(missing)
`,mo,ids],{encoding:'utf8'});
    assert.equal(checked.status,0,checked.stderr);
  }finally{await rm(directory,{recursive:true,force:true});}
});
