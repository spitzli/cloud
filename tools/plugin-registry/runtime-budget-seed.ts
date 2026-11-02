// Runs only inside the disposable EU budget fixture, never against production.
import { connectDB, db } from '/app/apps/api/src/db/index.ts';
import { initSettings } from '/app/apps/api/src/lib/settings.ts';
import { createKind } from '/app/apps/api/src/lib/kinds.ts';
const { migrate } = await import(Bun.resolveSync('drizzle-orm/bun-sqlite/migrator', '/app/apps/api'));
const fixtures = await Bun.file('/fixtures/fixtures.json').json();
await connectDB('sqlite:/data/budget-fixture.sqlite');
migrate(db, { migrationsFolder: '/app/apps/api/src/db/migrations' });
await initSettings();
await createKind({ key: 'extension', label: 'Extensions', description: 'Isolated budget fixture', manifestSchema: fixtures.extensionSchema });
await createKind(fixtures.providerKind);
await Bun.write('/data/config.json', JSON.stringify({ installed: true, database: { url: 'sqlite:/data/budget-fixture.sqlite' } }));
