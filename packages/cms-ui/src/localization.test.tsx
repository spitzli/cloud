import test from 'node:test';
import assert from 'node:assert/strict';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {CMSApp} from './client';
import {I18nProvider} from '@spitzli/i18n/react';
const props={siteName:'Customer-authored site name',siteURL:'https://site.example',accountURL:'https://auth.example/account',accessURL:'https://auth.example/people'};
test('CMS can render without any outer i18n provider and preserves customer copy',()=>{
 const html=renderToStaticMarkup(<CMSApp {...props}/>);
 assert.ok(html.includes('Your website, in your hands.'));assert.ok(html.includes(props.siteName));assert.ok(html.includes('Opening your content'));
});
test('explicit UI locale has its own boundary and cannot be replaced by an outer provider',()=>{
 const english=renderToStaticMarkup(<I18nProvider locale="de" preference="de"><CMSApp {...props} uiLocale="en"/></I18nProvider>);
 assert.ok(english.includes('Your website, in your hands.'));
 const german=renderToStaticMarkup(<CMSApp {...props} uiLocale="de"/>);
 assert.ok(german.includes('Deine Website. In deiner Hand.'));assert.ok(german.includes(props.siteName));
});
test("embedded CMS renders no navigation links out of its sandbox", () => {
  const html = renderToStaticMarkup(<CMSApp {...props} embedded />);
  assert.equal(html.includes('href="/cms"'), false);
  assert.equal(html.includes('href="https://site.example"'), false);
  assert.equal(html.includes("My account"), false);
});
