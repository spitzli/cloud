-- Native proofs are independent of retained external-provider references.
ALTER TABLE webdock_auth.database_launch ADD COLUMN IF NOT EXISTS native_authority jsonb;
ALTER TABLE webdock_auth.database_session ADD COLUMN IF NOT EXISTS native_authority jsonb;
ALTER TABLE webdock_auth.managed_database_launch ADD COLUMN IF NOT EXISTS native_authority jsonb;
ALTER TABLE webdock_auth.managed_database_session ADD COLUMN IF NOT EXISTS native_authority jsonb;
DO $$
DECLARE table_name text;
BEGIN
 FOREACH table_name IN ARRAY ARRAY['database_launch','database_session','managed_database_launch','managed_database_session'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I DROP CONSTRAINT IF EXISTS %I',table_name,table_name||'_native_authority_shape');
   EXECUTE format('ALTER TABLE webdock_auth.%I ADD CONSTRAINT %I CHECK(native_authority IS NULL OR (jsonb_typeof(native_authority)=''object'' AND coalesce(native_authority->>''kind'' IN (''local-session'',''first-party-browser''),false) AND session_id IS NOT NULL AND clerk_identity IS NULL))',table_name,table_name||'_native_authority_shape');
 END LOOP;
END $$;
