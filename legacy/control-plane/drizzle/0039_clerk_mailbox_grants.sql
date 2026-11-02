-- Native mailbox control tables are optional until explicitly enabled.
DO $$ BEGIN
 IF to_regclass('webdock_mail.mailbox_grant') IS NOT NULL THEN
-- Explicit provider-qualified grants; no shadow legacy membership or operator escalation.
ALTER TABLE webdock_mail.mailbox_grant ADD COLUMN IF NOT EXISTS clerk_grant text;
DROP TRIGGER IF EXISTS mail_member_detached_before ON webdock_mail.mailbox_grant;
CREATE TRIGGER mail_member_detached_before BEFORE UPDATE OF member_id ON webdock_mail.mailbox_grant
 FOR EACH ROW WHEN (OLD.member_id IS NOT NULL AND NEW.member_id IS NULL AND NEW.clerk_grant IS NULL) EXECUTE FUNCTION webdock_mail.revoke_detached_member();
DROP TRIGGER IF EXISTS mail_member_detached_after ON webdock_mail.mailbox_grant;
CREATE TRIGGER mail_member_detached_after AFTER UPDATE OF member_id ON webdock_mail.mailbox_grant
 FOR EACH ROW WHEN (OLD.member_id IS NOT NULL AND NEW.member_id IS NULL AND NEW.clerk_grant IS NULL) EXECUTE FUNCTION webdock_mail.revoke_detached_member();

 END IF;
END $$;
