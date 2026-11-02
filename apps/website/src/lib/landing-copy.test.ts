import test from 'node:test';
import assert from 'node:assert/strict';
import {landingDefaults} from '../cms/landing';
import {localizeLanding} from './landing-copy';
import {translator} from '@webdock/i18n';
import legacy from './legacy-landing.json';
test('known Webdock marketing copy is translated without mutating stored or authored content',()=>{
 const source={...landingDefaults,id:17,heroTitle:'A customer-authored headline',contactEmail:'custom@example.test',faqs:[{id:'custom',question:'My own question',answer:'My own answer'},{id:'default',...landingDefaults.faqs[0]}],useCases:[{id:'case',...landingDefaults.useCases[1]}]};
 const localized=localizeLanding(source,message=>'translated:'+message);
 assert.equal(localized.heroTitle,source.heroTitle);assert.equal(localized.contactEmail,source.contactEmail);assert.equal(localized.id,17);
 assert.equal(localized.heroDescription,'translated:'+landingDefaults.heroDescription);assert.equal(localized.seoTitle,'translated:'+landingDefaults.seoTitle);
 assert.deepEqual(localized.faqs[0],source.faqs[0]);assert.equal(localized.faqs[1].question,'translated:'+landingDefaults.faqs[0].question);assert.equal(localized.faqs[1].id,'default');
 assert.equal(localized.useCases[0].description,'translated:'+landingDefaults.useCases[1].description);assert.equal(source.faqs[1].question,landingDefaults.faqs[0].question);
});

test('compiled German catalog translates known hero and FAQ text with English fallback intact',()=>{
 const translated=localizeLanding(landingDefaults,translator('de').t);
 assert.equal(translated.heroTitle,'Deine Projekte.\nEin Ort.');assert.equal(translated.faqs[0].question,'Was ist Spitzli Cloud?');
 assert.deepEqual(localizeLanding(landingDefaults,translator('en').t),landingDefaults);
});

test('retired Studio announcement defaults upgrade only known values and intact FAQ pairs',()=>{
 const oldQuestion='Is there a client console yet?';const oldAnswer='Not yet. A dedicated console for managing projects is planned for a future release. Until then, Dominik is your direct point of contact.';
 const source={...landingDefaults,outlookTitle:'Your link today. Your dashboard tomorrow.',outlookDescription:'A dedicated console for your projects is planned. Until then, we’ll take care of things personally.',faqs:[{id:'known',question:oldQuestion,answer:oldAnswer},{id:'authored',question:oldQuestion,answer:'An individually authored answer'}]};
 const current=localizeLanding(source,message=>message);
 assert.equal(current.outlookTitle,'Your projects. One workspace.');assert.equal(current.outlookDescription,landingDefaults.outlookDescription);
 assert.deepEqual(current.faqs[0],{id:'known',...landingDefaults.faqs[landingDefaults.faqs.length-1]});assert.deepEqual(current.faqs[1],source.faqs[1]);
 const authored=localizeLanding({...source,outlookTitle:'Custom title',outlookDescription:'Custom copy'},message=>'translated:'+message);
 assert.equal(authored.outlookTitle,'Custom title');assert.equal(authored.outlookDescription,'Custom copy');assert.equal(source.faqs[0].answer,oldAnswer);
});

test('rebrand upgrades exact old defaults and intact pairs while retaining authored copy and IDs',()=>{
 const source={...legacy,id:17,contactEmail:'authored@example.test',faqs:[{id:'known',...legacy.faqs[0]},{id:'authored',question:legacy.faqs[0].question,answer:'An individually authored answer'}],useCases:[{id:'case',...legacy.useCases[1]}]};
 const current=localizeLanding(source,message=>message);
 assert.equal(current.heroTitle,'Your projects.\nOne place.');
 assert.equal(current.id,17);assert.equal(current.contactEmail,source.contactEmail);
 assert.equal(current.faqs[0].question,'What is Spitzli Cloud?');assert.equal(current.faqs[0].id,'known');
 assert.deepEqual(current.faqs[1],source.faqs[1]);
 assert.equal(current.useCases[0].id,'case');assert.match(current.useCases[0].description,/spitzli\.site/);
 assert.equal(source.heroTitle,legacy.heroTitle);assert.equal(source.faqs[0].question,legacy.faqs[0].question);
});
