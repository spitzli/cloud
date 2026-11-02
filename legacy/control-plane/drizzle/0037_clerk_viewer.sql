-- Preserve the legacy FK. A provider-qualified reference is mutually exclusive.
DO $$ DECLARE name text; BEGIN
 FOREACH name IN ARRAY ARRAY['database_launch','database_session','managed_database_launch','managed_database_session'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I ALTER COLUMN session_id DROP NOT NULL, ADD COLUMN clerk_identity jsonb',name);
  EXECUTE format('ALTER TABLE webdock_auth.%I ADD CONSTRAINT %I CHECK (coalesce((session_id IS NOT NULL AND clerk_identity IS NULL) OR (session_id IS NULL AND jsonb_typeof(clerk_identity)=''object'' AND clerk_identity ?& ARRAY[''issuer'',''userID'',''sessionID'',''organizationID''] AND jsonb_typeof(clerk_identity->''issuer'')=''string'' AND jsonb_typeof(clerk_identity->''userID'')=''string'' AND jsonb_typeof(clerk_identity->''sessionID'')=''string'' AND (clerk_identity->''organizationID''=''null''::jsonb OR jsonb_typeof(clerk_identity->''organizationID'')=''string'') AND length(clerk_identity->>''issuer'')>0 AND length(clerk_identity->>''userID'')>0 AND length(clerk_identity->>''sessionID'')>0),false))',name,name||'_identity_provider');
 END LOOP;
END $$;
