import { test } from 'node:test';
import assert from 'node:assert/strict';
import { projectLink } from '../src/project-links';
await test('saved project links allow HTTPS without credentials and never activate executable URLs', () => {
  assert.equal(projectLink('https://admin.example.test/path'), 'https://admin.example.test/path');
  for (const value of [null, undefined, '', 'javascript:alert(1)', 'data:text/html,x', 'http://example.test', 'https://secret@example.test', 'https://u:p@example.test', '/relative']) assert.equal(projectLink(value), null);
});
