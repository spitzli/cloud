// Acceptance-only: migrate a fresh owned database to the old schema before seeding bytes.
import assert from "node:assert/strict";
import { randomUUID, createHash } from "node:crypto";
import { writeFile } from "node:fs/promises";
import sharp from "sharp";
import pg from "pg";
import { getPayload } from "payload";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { loadCMSConfig } from "../src/lib/config.ts";
import config from "../src/payload.config.ts";
import { migrations } from "../src/migrations/index.ts";
const environment = loadCMSConfig();
assert.equal(environment.migrating, true);
assert.match(environment.instance, /^fixture-[a-f0-9]{16}$/);
assert.equal(environment.mediaStorage.kind, "local");
process.env.PAYLOAD_MIGRATING = "true";
const payload = await getPayload({ config, disableOnInit: true });
const client = new pg.Client(
  verifiedPostgresConfig(environment.databaseURL, environment.databaseCA),
);
try {
  await payload.db.migrate({
    migrations: migrations.slice(0, 2) as never,
    shouldPrompt: false,
  });
  await client.connect();
  const columns = await client.query(
    "SELECT column_name FROM information_schema.columns WHERE table_schema='app' AND table_name='media' AND column_name='content_sha256'",
  );
  assert.equal(columns.rowCount, 0);
  const bytes = await sharp({
    create: { width: 3, height: 3, channels: 3, background: "#abc" },
  })
    .png()
    .toBuffer();
  const filename = `${randomUUID()}.png`;
  const url = `${environment.origin}/api/media/file/${filename}`;
  const row = (
    await client.query(
      "INSERT INTO app.media(filename,mime_type,filesize,width,height,url) VALUES($1,'image/png',$2,3,3,$3) RETURNING id,created_at,updated_at",
      [filename, bytes.length, url],
    )
  ).rows[0];
  await client.query(
    "INSERT INTO app.media_locales(alt,_locale,_parent_id) VALUES('Preexisting local image','en',$1)",
    [row.id],
  );
  const version = (
    await client.query(
      "INSERT INTO app._media_v(parent_id,version_filename,version_mime_type,version_filesize,version_url,version_created_at,version_updated_at) VALUES($1,$2,'image/png',$3,$4,$5,$6) RETURNING id",
      [row.id, filename, bytes.length, url, row.created_at, row.updated_at],
    )
  ).rows[0];
  await client.query(
    "INSERT INTO app._media_v_locales(version_alt,_locale,_parent_id) VALUES('Preexisting local image','en',$1)",
    [version.id],
  );
  await writeFile(`${environment.mediaDirectory}/${filename}`, bytes);
  await writeFile(
    `${environment.transferDirectory}/legacy-media.json`,
    JSON.stringify({
      ...row,
      filename,
      url,
      size: bytes.length,
      version: version.id,
      hash: createHash("sha256").update(bytes).digest("hex"),
    }),
  );
} finally {
  await client.end();
  await payload.destroy();
}
