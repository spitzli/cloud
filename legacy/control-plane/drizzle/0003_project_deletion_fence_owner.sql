-- The fence function runs as the non-login owner. FORCE RLS still requires an
-- explicit policy, including when the journal was created after RLS installation.
CREATE POLICY api_deletion_owner ON webdock_admin.project_deletion
 TO webdock_platform_owner USING (true) WITH CHECK (true);
