import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,rmSync,readdirSync,readFileSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {execFileSync} from 'node:child_process';
import ts from 'typescript';
import {createElement} from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {I18nProvider} from '@webdock/i18n/react';
import {translator} from '@webdock/i18n';
import {AuthPanel} from '../src/components/auth-forms';
import {authMessage} from '../src/lib/i18n-feedback';
import {authLabel} from '../src/lib/i18n-labels';
import {signInError,SIGN_IN_EXPIRED} from '../src/lib/sign-in-recovery';
import {POST} from '../src/app/api/locale/route';

test('Auth preference updates preserve the signed URL and never write an identity cookie',async()=>{
 const origin=new URL(process.env.BETTER_AUTH_URL||'http://localhost:3125').origin;
 const url=origin+'/api/locale?sig=untouched&nonce=keep%2Bencoding&client_id=123';
 const req=new Request(url,{method:'POST',headers:{Origin:origin,'Content-Type':'application/json'},body:JSON.stringify({preference:'de'})});
 const response=await POST(req);assert.equal(response.status,204);assert.equal(req.url,url);assert.equal(response.headers.has('location'),false);
 assert.equal(response.headers.getSetCookie().length,1);assert.match(response.headers.get('set-cookie')!,/^webdock_locale=de;/);
 assert.equal(signInError({code:'invalid_signature'},'fallback'),SIGN_IN_EXPIRED);
 assert.equal(signInError({message:'Invalid email or password'},'fallback'),'Invalid email or password','Native response/error state remains unchanged');
});
test('Auth presentation translates enum labels without changing values or authored names',()=>{
 const en=translator('en');assert.equal(authLabel('admin',en.t),'Tenant admin');assert.equal(authLabel('customer-authored-value',en.t),'customer-authored-value');
 assert.equal(authMessage('Connection successful. 1 subaccount available.',en),'Connection successful. 1 subaccount available.');
 assert.equal(authMessage('Connection successful. 2 subaccounts available.',en),'Connection successful. 2 subaccounts available.');
 const html=renderToStaticMarkup(createElement(I18nProvider,{locale:'en',preference:'system'},createElement(AuthPanel,{title:'Sign in to Webdock'} as Parameters<typeof AuthPanel>[0],'Unchanged customer text <name>')));
 assert.ok(html.includes('Sign in to Webdock'));assert.ok(html.includes('Unchanged customer text &lt;name&gt;'));
});
test('GNU Auth catalog covers every extracted source and supplies reviewed security translations',()=>{
 const temp=mkdtempSync(join(tmpdir(),'webdock-auth-i18n-')),files:string[]=[];
 const scan=(dir:string)=>{for(const f of readdirSync(dir,{withFileTypes:true})){const p=join(dir,f.name);if(f.isDirectory())scan(p);else if(/\.tsx?$/.test(p))files.push(p)}};scan(resolve('src'));
 try{
  execFileSync('xgettext',['--from-code=UTF-8','--keyword','--keyword=t:1','--keyword=msgid:1','--keyword=n:1,2','--keyword=p:1c,2','--no-wrap','-o',join(temp,'auth.pot'),...files]);
  execFileSync('msgen',[join(temp,'auth.pot'),'-o',join(temp,'source.po')]);writeFileSync(join(temp,'source.po'),readFileSync(join(temp,'source.po'),'utf8').replace('nplurals=INTEGER; plural=EXPRESSION;','nplurals=2; plural=(n != 1);'));execFileSync('msgfmt',[join(temp,'source.po'),'-o',join(temp,'source.mo')]);
  execFileSync('msgfmt',['--check','--check-format',resolve('../../packages/i18n/locales/de/auth.po'),'-o',join(temp,'de.mo')]);
  execFileSync('python3',['-c',`import gettext,sys
with open(sys.argv[1],'rb') as f:source=gettext.GNUTranslations(f)._catalog
with open(sys.argv[2],'rb') as f:de=gettext.GNUTranslations(f)
missing=[k for k in source if k and (k not in de._catalog or not de._catalog[k])]
assert not missing, missing
assert de.gettext('Sign in to Webdock')=='Bei Webdock anmelden'
assert de.gettext('Authenticator code')=='Authenticator-Code'
assert de.gettext('This sign-in request has expired or is no longer valid. Start a new sign-in from Studio.').startswith('Diese Anmeldeanfrage')
assert de.ngettext('Connection successful. {count} subaccount available.','Connection successful. {count} subaccounts available.',2)=='Verbindung erfolgreich. {count} Unterkonten verfügbar.'
`,join(temp,'source.mo'),join(temp,'de.mo')]);
  const forms=readFileSync(resolve('src/components/auth-forms.tsx'),'utf8');assert.equal((forms.match(/error === SIGN_IN_EXPIRED/g)||[]).length,2,'Raw signature-expiry comparisons remain semantic');
 }finally{rmSync(temp,{recursive:true,force:true})}
});

test('Translations never enter stored form values, action identifiers or URLs',()=>{
 const scan=(dir:string)=>{for(const entry of readdirSync(dir,{withFileTypes:true})){const file=join(dir,entry.name);if(entry.isDirectory())scan(file);else if(file.endsWith('.tsx')){
  const source=ts.createSourceFile(file,readFileSync(file,'utf8'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
  const inspect=(node:ts.Node)=>{if(ts.isJsxAttribute(node)&&['value','defaultValue','name','href','method','type','pattern','autoComplete','htmlFor','id'].includes(node.name.getText(source))&&node.initializer){
   const check=(child:ts.Node)=>{if(ts.isCallExpression(child))assert.ok(!['t','authLabel','translateError'].includes(child.expression.getText(source)),`Translation in non-display attribute ${file}: ${node.getText(source)}`);ts.forEachChild(child,check)};check(node.initializer);
  }ts.forEachChild(node,inspect)};inspect(source);
 }}};scan(resolve('src'));
});
