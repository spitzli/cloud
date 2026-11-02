ALTER TABLE webdock_auth.plugin_source_import
 ADD COLUMN actions_metadata jsonb CHECK(actions_metadata IS NULL OR (jsonb_typeof(actions_metadata)='object' AND octet_length(actions_metadata::text)<=8192));
