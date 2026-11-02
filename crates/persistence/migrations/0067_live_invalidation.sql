-- Fixed, non-sensitive invalidation counters only. Tenant rows stay behind signed RLS/API reads.
CREATE SCHEMA webdock_live AUTHORIZATION webdock_platform_owner;
REVOKE ALL ON SCHEMA webdock_live FROM PUBLIC;
CREATE TABLE webdock_live.change_revision (
 topic text PRIMARY KEY CHECK(topic IN ('workspace','operator-customers')),
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0)
);
ALTER TABLE webdock_live.change_revision OWNER TO webdock_platform_owner;
REVOKE ALL ON webdock_live.change_revision FROM PUBLIC;
INSERT INTO webdock_live.change_revision(topic) VALUES('workspace'),('operator-customers');
CREATE FUNCTION webdock_live.invalidate_directory() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
BEGIN
 UPDATE webdock_live.change_revision SET revision=revision+1;
 RETURN NULL;
END $$;
ALTER FUNCTION webdock_live.invalidate_directory() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_live.invalidate_directory() FROM PUBLIC;
DO $$ DECLARE relation text; reader text; BEGIN
 FOREACH relation IN ARRAY ARRAY[
  'webdock_admin.customers','webdock_auth.organization','webdock_auth.member',
  'webdock_auth.invitation','webdock_auth.tenant_customer','webdock_auth.project_grant',
  'webdock_auth.app_binding','webdock_auth.clerk_identity','webdock_auth.clerk_identity_mapping',
  'webdock_auth.clerk_organization','webdock_auth.local_identity_activation',
  'webdock_auth.local_organization_activation','webdock_auth.access_invitation_state'
 ] LOOP
  EXECUTE format('CREATE TRIGGER live_directory_revision AFTER INSERT OR UPDATE OR DELETE ON %s FOR EACH STATEMENT EXECUTE FUNCTION webdock_live.invalidate_directory()',relation);
 END LOOP;
 -- Exclude last-seen/session timestamps: authorized refresh reads must never trigger a loop.
 CREATE TRIGGER live_identity_revision AFTER INSERT OR DELETE OR UPDATE OF role,banned,"emailVerified","mustChangePassword","twoFactorEnabled" ON webdock_auth."user" FOR EACH STATEMENT EXECUTE FUNCTION webdock_live.invalidate_directory();
 SELECT pg_get_userbyid(datdba) INTO STRICT reader FROM pg_database WHERE datname=current_database();
 EXECUTE format('GRANT USAGE ON SCHEMA webdock_live TO %I',reader);
 EXECUTE format('GRANT SELECT ON webdock_live.change_revision TO %I',reader);
END $$;
