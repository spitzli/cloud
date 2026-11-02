ALTER TABLE webdock_auth.git_source DROP CONSTRAINT IF EXISTS git_source_project_id_key;
CREATE UNIQUE INDEX IF NOT EXISTS git_source_project_target_key ON webdock_auth.git_source(project_id,target_id);
ALTER TABLE webdock_auth.git_release ADD COLUMN IF NOT EXISTS source_id varchar;
-- The offline owner must backfill history even when the runtime tables force RLS.
-- Transactional DDL keeps other sessions fenced until the original flags are restored.
DO $backfill$
DECLARE forced text[]; relation_name text;
BEGIN
 SELECT array_agg(c.relname::text) INTO forced FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname='webdock_auth' AND c.relname IN ('git_release','git_artifact','git_build') AND c.relforcerowsecurity;
 FOREACH relation_name IN ARRAY coalesce(forced,ARRAY[]::text[]) LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I NO FORCE ROW LEVEL SECURITY',relation_name);
 END LOOP;
 UPDATE webdock_auth.git_release r SET source_id=b.source_id FROM webdock_auth.git_artifact a JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE r.artifact_id=a.id AND r.source_id IS NULL;
 FOREACH relation_name IN ARRAY coalesce(forced,ARRAY[]::text[]) LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',relation_name);
 END LOOP;
END $backfill$;
ALTER TABLE webdock_auth.git_release ALTER COLUMN source_id SET NOT NULL;
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conname='git_release_source_owner_fk' AND conrelid='webdock_auth.git_release'::regclass) THEN
 ALTER TABLE webdock_auth.git_release ADD CONSTRAINT git_release_source_owner_fk FOREIGN KEY(source_id,customer_id,project_id) REFERENCES webdock_auth.git_source(id,customer_id,project_id);
 END IF;
END $$;
DROP INDEX IF EXISTS webdock_auth.git_release_publication;
CREATE UNIQUE INDEX IF NOT EXISTS git_release_source_publication ON webdock_auth.git_release(source_id) WHERE status IN ('deploying','needs-reconciliation');
