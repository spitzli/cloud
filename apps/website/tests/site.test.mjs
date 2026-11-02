import test from 'node:test';
import assert from 'node:assert/strict';
const origin = process.env.TEST_BASE_URL || 'http://localhost:3104';
test('Landing page serves English content, matching FAQ data and SEO assets', async () => {
  const response = await fetch(origin);
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /<html[^>]+lang="en"/);
  assert.match(html, /<link rel="canonical" href="https:\/\/spitzli\.cloud\/?"/);
  assert.match(html, /href="https:\/\/spitzli.dev"/);
  assert.match(html, /<a[^>]+href="https:\/\/console\.spitzli\.cloud\/waitlist"[^>]*>Waitlist/);
  assert.match(html, /<a[^>]+href="https:\/\/console\.spitzli\.cloud"[^>]*>Login/);
  assert.doesNotMatch(html, />Register/);
  assert.match(html, /class="button" href="https:\/\/console\.spitzli\.cloud\/waitlist">Join the waitlist/);
  assert.doesNotMatch(html, /moon\.webdock\.dev|id="managed-analytics"/);
  assert.doesNotMatch(html, /class="spitzli-section"|Meet Spitzli|A human on your side/);
  assert.match(html, /Access by invitation only\./);
  assert.doesNotMatch(html, /href="https:\/\/accounts\.spitzli\.cloud\/sign-up"/);
  assert.match(html, /href="mailto:[^"]+"/);
  assert.equal((html.match(/<h1[ >]/g) || []).length, 1);
  const data = JSON.parse(html.match(/<script type="application\/ld\+json">(.*?)<\/script>/s)[1]);
  const faq = data['@graph'].find(item => item['@type'] === 'FAQPage');
  assert.ok(faq.mainEntity.length > 0);
  assert.equal((html.match(/<details[ >]/g) || []).length, faq.mainEntity.length);
  const visibleFAQ = html.match(/<section id="faq"[\s\S]*?<\/section>/)[0];
  const escapeHTML = value => value.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#x27;' })[char]);
  for (const item of faq.mainEntity) {
    assert.ok(visibleFAQ.includes(escapeHTML(item.name)));
    assert.ok(visibleFAQ.includes(escapeHTML(item.acceptedAnswer.text)));
  }
  for (const path of ['/robots.txt', '/sitemap.xml', '/icon.svg', '/og', '/favicon.ico']) {
    const asset = await fetch(origin + path);
    assert.equal(asset.status, 200, path);
    assert.ok((await asset.arrayBuffer()).byteLength > 30, path);
  }
  assert.match(await (await fetch(origin + '/sitemap.xml')).text(), /https:\/\/spitzli.cloud/);
});
