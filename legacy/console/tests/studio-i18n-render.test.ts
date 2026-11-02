import test from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { I18nProvider } from '@webdock/i18n/react';
import { Editor } from '../src/components/editor';

test('localized editor labels never translate customer-authored option names or field values', () => {
  const html = renderToStaticMarkup(createElement(I18nProvider, {locale:'de',preference:'de'}, createElement(Editor, {
    action: async () => ({}),
    fields: [
      {name:'customer',label:'Language',type:'select',value:'customer-123',options:[{value:'customer-123',label:'Language',translate:false}]},
      {name:'name',label:'Language',value:'Language'},
    ],
  })));
  assert.match(html,/Sprache/);
  assert.match(html,/<option value="customer-123" selected="">Language<\/option>/);
  assert.match(html,/name="name"[^>]*value="Language"/);
  assert.doesNotMatch(html,/value="Sprache"/);
});
