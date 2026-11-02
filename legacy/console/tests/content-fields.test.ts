import test from 'node:test';
import assert from 'node:assert/strict';
import { parseContentField, contentSections } from '../src/lib/content-fields';
const field = {key:'modern_text_001',variant:'modern',kind:'text',label:'Title',value:'Hello',updatedAt:'2026-10-12'};
test('content grouping uses explicit source metadata and preserves legacy fields', () => {
  assert.equal(parseContentField(field).section, undefined);
  assert.equal(parseContentField({...field,section:'Startseite / Kontakt'}).section, 'Startseite / Kontakt');
  assert.deepEqual(contentSections([parseContentField(field)]), []);
  assert.deepEqual(contentSections([parseContentField({...field,section:'Kontakt'}),parseContentField({...field,section:'Startseite'}),parseContentField({...field,section:'Kontakt'})]), ['Kontakt','Startseite']);
});
test('invalid content grouping metadata cannot cross the bridge', () => {
  for(const section of [42, {}, '', 'x'.repeat(161)]) assert.throws(()=>parseContentField({...field,section}));
  assert.throws(()=>parseContentField({...field,key:'other_field'}));
});
