import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,readFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
const compiler=fileURLToPath(new URL('../scripts/compile.py',import.meta.url));
const header=`msgid ""\nmsgstr ""\n"Project-Id-Version: Test\\n"\n"Language: de\\n"\n"MIME-Version: 1.0\\n"\n"Content-Type: text/plain; charset=UTF-8\\n"\n"Content-Transfer-Encoding: 8bit\\n"\n"Plural-Forms: nplurals=2; plural=(n != 1);\\n"\n`;
test('GNU catalogs compile context/plurals and reject conflicts or broken named placeholders',()=>{
 const dir=mkdtempSync(join(tmpdir(),'webdock-i18n-test-'));const output=join(dir,'compiled.json');
 const run=()=>spawnSync('python3',[compiler,'--catalog-dir',dir,'--output',output],{encoding:'utf8'});
 try{
  writeFileSync(join(dir,'auth.po'),header+'\nmsgid "Untranslated source"\nmsgstr ""\n\nmsgctxt "verb"\nmsgid "Open"\nmsgstr "Öffnen"\n\nmsgid "{count} page"\nmsgid_plural "{count} pages"\nmsgstr[0] "{count} Seite"\nmsgstr[1] "{count} Seiten"\n');
  let result=run();assert.equal(result.status,0,result.stderr);assert.deepEqual(JSON.parse(readFileSync(output,'utf8')),{'Untranslated source':[''],'verb\u0004Open':['Öffnen'],'{count} page':['{count} Seite','{count} Seiten']});
  writeFileSync(join(dir,'plural.po'),header+'\nmsgid "One result"\nmsgid_plural "{count} results"\nmsgstr[0] "Ein Ergebnis"\nmsgstr[1] "{count} Ergebnisse"\n');result=run();assert.equal(result.status,0,result.stderr);
  writeFileSync(join(dir,'studio.po'),header+'\nmsgctxt "verb"\nmsgid "Open"\nmsgstr "Offen"\n');result=run();assert.notEqual(result.status,0);assert.match(result.stderr,/Conflicting translation/);
  writeFileSync(join(dir,'studio.po'),header+'\nmsgid "Hello {name}"\nmsgstr "Hallo {person}"\n');result=run();assert.notEqual(result.status,0);assert.match(result.stderr,/placeholder mismatch/);
 }finally{rmSync(dir,{recursive:true,force:true})}
});
test('Checked-in generated catalog matches authoritative PO files',{timeout:120_000},()=>{
 const result=spawnSync('python3',[compiler,'--check'],{encoding:'utf8'});assert.equal(result.status,0,result.stderr);
});
