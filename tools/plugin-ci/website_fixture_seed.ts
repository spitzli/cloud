// Manual owned-fixture helper. It does not alter website modules or accept a remote DB.
import assert from 'node:assert/strict';
assert.equal(process.env.WEBDOCK_OWNED_WEBSITE_FIXTURE, '1');
const target = new URL(process.env.DATABASE_URL || '');
assert.equal(target.hostname, '127.0.0.1');
assert.equal(target.username, 'quality');
assert.match(target.pathname, /^\/owned_website_[a-f0-9]{32}$/);
assert.ok(target.port);
const { getPayload } = await import('/src/node_modules/payload/dist/index.js');
const config = (await import('/src/apps/website/src/payload.config.ts')).default;
const { migrations } = await import('/src/apps/website/src/migrations-instance/index.ts');
const { landingDefaults } = await import('/src/apps/website/src/cms/landing.ts');
const payload = await getPayload({ config, disableOnInit: true });
try {
  const result = await payload.db.migrate({ migrations, shouldPrompt: false });
  assert.ok(!result?.cancelled);
  await payload.updateGlobal({ slug: 'landing-page', data: landingDefaults, overrideAccess: true });
  const page = await payload.findGlobal({ slug: 'landing-page', overrideAccess: true, depth: 0 });
  assert.equal(page.heroTitle, landingDefaults.heroTitle);
  assert.equal(page.faqs.length, landingDefaults.faqs.length);
  console.log('Owned Payload schema migrations and authored landing defaults seeded.');
} finally { await payload.destroy(); }
