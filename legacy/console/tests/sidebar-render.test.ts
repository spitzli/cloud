import { ModuleProvider } from "@webdock/module-sdk/react";
import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement as h } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { PathnameContext, SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime';
import { I18nProvider } from '@webdock/i18n/react';
import { MailNavigation } from '../src/components/mail/mail-navigation';
import { Navigation, TenantNavigation } from '../src/components/navigation';
import { AreaNavigation } from '../src/components/area-navigation';

function render(path: string, element: ReturnType<typeof h>, query = '', state: 'enabled' | 'draining' | 'disabled' = 'enabled') {
  element = h(ModuleProvider, {view:{revision:state, modules:{mail:state,hosting:"enabled",databases:"enabled",cms:"enabled",storage:"enabled",billing:"enabled"}}, children:element});
  return renderToStaticMarkup(h(I18nProvider, {locale:'en',preference:'en'}, h(PathnameContext.Provider,{value:path},h(SearchParamsContext.Provider,{value:new URLSearchParams(query)},element))));
}
test('Mail uses one area group and highlights only the current sending tool', () => {
  const html = render('/tenants/12/mail/emails/42', h(MailNavigation,{customerID:'12',canManage:true}), 'connection=34');
  assert.equal((html.match(/class="area-navigation"/g)||[]).length,1);
  assert.equal((html.match(/aria-current="page"/g)||[]).length,2); // Desktop and mobile render the same selected destination.
  assert.match(html,/href="\/tenants\/12\/mail\/emails\?connection=34"/);
  assert.match(html,/area-link-nested/);
  assert.doesNotMatch(html,/Sending tools/);
});
test('Mail hides administration tools from members without losing inbox and domains', () => {
  const html=render('/tenants/12/mail/emails',h(MailNavigation,{customerID:'12',canManage:false}));
  assert.doesNotMatch(html,/\/api-keys|\/webhooks|\/suppressions|\/settings|\/tracking/);
  assert.match(html,/\/mailboxes/);assert.match(html,/\/domains/);
});
test('workspace navigation preserves database and preview access rules', () => {
  const html=render('/tenants/12',h(TenantNavigation,{customerID:'12',canManage:true}));
  assert.match(html,/\/databases/);assert.doesNotMatch(html,/\/billing/);assert.match(html,/\/team/);
  const preview=render('/tenants/12',h(TenantNavigation,{customerID:'12',canManage:false,preview:true}));
  assert.match(preview,/\/managed-databases/);assert.doesNotMatch(preview,/\/mail|\/billing|\/team/);
});
test('website sections retain their exact query selection in the sidebar',()=>{
  const html=render('/sites/12/shop',h(AreaNavigation,{links:[{href:'/sites/12/shop?kind=products',label:'Products',exact:true},{href:'/sites/12/shop?kind=orders',label:'Orders',exact:true}]}),'kind=orders');
  assert.equal((html.match(/aria-current="page"/g)||[]).length,2);
  assert.doesNotMatch(html,/href="\/sites\/12\/shop\?kind=products"[^>]*aria-current/);
});

test('installation state hides Mail without removing other workspace services', () => {
 const disabled=render('/tenants/12',h(TenantNavigation,{customerID:'12',canManage:true}),'','disabled');
 assert.doesNotMatch(disabled,/\/mail/);
 assert.match(disabled,/\/databases/);
 assert.equal(render('/tenants/12/mail',h(MailNavigation,{customerID:'12',canManage:true}),'','disabled'),'');
 assert.match(render('/tenants/12',h(TenantNavigation,{customerID:'12'}),'','draining'),/\/mail/);
});

test('minimal installation hides product navigation but preserves workspace access', () => {
 const element=h(ModuleProvider,{view:{revision:'minimal',modules:{mail:'disabled',hosting:'disabled',databases:'disabled',cms:'disabled',storage:'disabled',billing:'disabled'}},children:h(TenantNavigation,{customerID:'12',canManage:true})});
 const html=render('/tenants/12',element);
 assert.doesNotMatch(html,/href="[^"]*\/(?:hosting|databases|mail|billing)"/);
 assert.match(html,/\/team/);assert.match(html,/\/profile/);
});

test('empty installation renders the actual admin navigation without product links', () => {
 const element=h(ModuleProvider,{view:{revision:'core',modules:{}},children:h(Navigation,{operator:true})});
 const html=render('/',element);
 assert.match(html,/href="\/"/);assert.match(html,/href="\/customers"/);assert.match(html,/href="\/people"/);
 assert.doesNotMatch(html,/href="[^\"]*(?:hosting|sites|plans|integrations|operations|usage)/);
});

test('selected Clerk core links only to local registry accounts, without legacy provisioning', () => {
 const element=h(ModuleProvider,{view:{revision:'core',modules:{}},children:h(Navigation,{operator:true,accountProvider:'clerk'})});
 const html=render('/',element);
 assert.match(html,/href="\/admin\/registry-users"/);
 assert.doesNotMatch(html,/href="\/people"|href="[^\"]*(?:delete|hosting|sites|plans|integrations|operations)/);
});
