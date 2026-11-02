import { type MigrateUpArgs, sql } from "@payloadcms/db-postgres";

export async function up({ db }: MigrateUpArgs): Promise<void> {
  await db.execute(sql`
    ALTER TABLE app.media ADD COLUMN content_sha256 varchar;
    ALTER TABLE app.media ADD COLUMN prefix varchar DEFAULT '';
    ALTER TABLE app.media ADD COLUMN _objectkey varchar;
    ALTER TABLE app._media_v ADD COLUMN version_content_sha256 varchar;
    ALTER TABLE app._media_v ADD COLUMN version_prefix varchar DEFAULT '';
    ALTER TABLE app._media_v ADD COLUMN version__objectkey varchar;
    CREATE TABLE app.cms_media_storage (
      singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
      binding jsonb NOT NULL CHECK (jsonb_typeof(binding) = 'object')
    );
    CREATE FUNCTION app.cms_media_storage_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN RAISE EXCEPTION 'Media storage binding requires explicit migration reconciliation'; END $$;
    CREATE TRIGGER cms_media_storage_immutable BEFORE UPDATE OR DELETE ON app.cms_media_storage
      FOR EACH ROW EXECUTE FUNCTION app.cms_media_storage_immutable();
  `);
}
export async function down(): Promise<void> {
  throw Error(
    "Media storage rollback requires an explicit checked migration; retained objects are not deleted",
  );
}
