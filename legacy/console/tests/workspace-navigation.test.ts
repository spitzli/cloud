import test from 'node:test';
import assert from 'node:assert/strict';
import { mainArea, areaSelected, soleWorkspace, applicationState, mailSendingPaths } from '../src/lib/workspace-navigation';

test('all deep links retain exactly one main area without prefix collisions', () => {
  for (const path of ['/projects/12', '/hosting/projects/12/git/builds/34', '/hosting/apps/56/settings', '/hosting/databases/56/settings', '/hosting/databases/56/connection', '/sites/78/shop']) assert.equal(mainArea(path, true), 'projects');
  for (const path of ['/customers/12/hosting', '/tenants/12/mail/keys', '/tenants/12/databases/34']) assert.equal(mainArea(path, true), 'customers');
  for (const path of ['/operations', '/infrastructure/12', '/hosting/operations/12', '/activity']) assert.equal(mainArea(path, true), 'operations');
  assert.equal(mainArea('/people-extra', true), null);
  assert.equal(mainArea('/admin/plans', true), 'plans');
  assert.equal(mainArea('/tenants/12/mail', false), 'workspace');
});
test('area selection respects path boundaries and query-based shop modules', () => {
  assert.equal(areaSelected('/sites/12/shop?kind=products', '/sites/12/shop', ''), true);
  assert.equal(areaSelected('/sites/12/shop?kind=products', '/sites/12/shop', 'kind=orders'), false);
  assert.equal(areaSelected('/sites/12/shop?kind=orders', '/sites/12/shop', 'kind=orders&page=2'), true);
  assert.equal(areaSelected('/tenants/12/mail', '/tenants/12/mail/inbox', ''), true);
  assert.equal(areaSelected('/tenants/12', '/tenants/12/mail', '', true), false);
  assert.equal(areaSelected('/tenants/12/mail', '/tenants/12/mailing', ''), false);
  assert.equal(areaSelected('/tenants/12/mail/mailboxes', '/tenants/12/mail/inbox', '', false, ['/tenants/12/mail/inbox']), true);
});
test('single workspace landing never swallows invitations or operator selection', () => {
  const tenant = { id: '12', status: 'active' };
  assert.equal(soleWorkspace([tenant], 0, false), '/tenants/12');
  assert.equal(soleWorkspace([tenant], 1, false), null);
  assert.equal(soleWorkspace([tenant], 0, true), null);
  assert.equal(soleWorkspace([tenant, { id: '34', status: 'active' }], 0, false), null);
  assert.equal(soleWorkspace([{ ...tenant, status: 'archived' }], 0, false), null);
});
test('application state does not confuse running work with running applications', () => {
  assert.equal(applicationState('running'), 'Running');
  assert.equal(applicationState('pending'), 'Change in progress');
  assert.equal(applicationState('unexpected'), 'Unknown');
  assert.equal(applicationState('deleted'), 'Deleted');
});

test('Mail API deep links belong to Sending while inbox, domains and settings stay separate', () => {
 const base='/tenants/12/mail', paths=mailSendingPaths.map(path=>base+path);
 for(const suffix of ['/sending','/emails/42','/analytics','/quickstart','/api-keys','/webhooks/7','/suppressions','/logs/9'])
  assert.equal(areaSelected(base+'/sending',base+suffix,'connection=34',false,paths),true,suffix);
 for(const suffix of ['/inbox','/mailboxes','/domains','/settings','/tracking','/emails-extra'])
  assert.equal(areaSelected(base+'/sending',base+suffix,'',false,paths),false,suffix);
});

test('managed databases remain within the single tenant Databases area', () => {
  assert.equal(areaSelected('/tenants/12/databases','/tenants/12/managed-databases','',false,['/tenants/12/managed-databases']),true);
  assert.equal(areaSelected('/tenants/12/databases','/tenants/123/managed-databases','',false,['/tenants/12/managed-databases']),false);
});
