ALTER TABLE webdock_auth.git_flow ADD COLUMN IF NOT EXISTS setup_id varchar;
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conname='git_flow_setup_id_fkey' AND conrelid='webdock_auth.git_flow'::regclass) THEN
 ALTER TABLE webdock_auth.git_flow ADD CONSTRAINT git_flow_setup_id_fkey FOREIGN KEY(setup_id) REFERENCES webdock_auth.hosting_setup(id);
 END IF;
END $$;
