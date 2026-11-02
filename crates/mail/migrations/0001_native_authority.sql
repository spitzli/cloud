-- Retained external sessions are not silently promoted to local authority. Their owners reopen
-- Webmail after authenticating through the native flow; existing rows/IDs remain unchanged.
ALTER TABLE webdock_mail.webmail_launch ADD COLUMN IF NOT EXISTS native_authority jsonb;
ALTER TABLE webdock_mail.webmail_session ADD COLUMN IF NOT EXISTS native_authority jsonb;
DO $$
DECLARE table_name text;
BEGIN
 FOREACH table_name IN ARRAY ARRAY['webmail_launch','webmail_session'] LOOP
  EXECUTE format('ALTER TABLE webdock_mail.%I DROP CONSTRAINT IF EXISTS %I',table_name,table_name||'_native_authority_check');
  EXECUTE format('ALTER TABLE webdock_mail.%I DROP CONSTRAINT IF EXISTS %I',table_name,table_name||'_native_authority_shape');
  EXECUTE format('ALTER TABLE webdock_mail.%I ADD CONSTRAINT %I CHECK(native_authority IS NULL OR (jsonb_typeof(native_authority)=''object'' AND coalesce(native_authority->>''kind'' IN (''local-session'',''first-party-browser''),false)))',table_name,table_name||'_native_authority_shape');
 END LOOP;
END $$;
