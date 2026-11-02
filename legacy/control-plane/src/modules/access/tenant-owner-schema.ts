/** The DB guard also covers Better Auth's operator ban endpoint, which does not use manageTenant. */
export const tenantOwnerSchemaSQL = `
CREATE OR REPLACE FUNCTION webdock_auth.guard_customer_owner() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,webdock_auth AS $$
DECLARE org text;
BEGIN
 IF TG_TABLE_NAME='member' THEN
  IF OLD.role<>'owner' THEN RETURN coalesce(NEW,OLD); END IF;
  IF TG_OP='UPDATE' AND NEW.role='owner' AND NEW."userId"=OLD."userId" AND NEW."organizationId"=OLD."organizationId" THEN RETURN NEW; END IF;
  IF NOT EXISTS(SELECT 1 FROM webdock_auth."user" WHERE id=OLD."userId" AND role='user' AND NOT coalesce(banned,false) AND "emailVerified" AND NOT "mustChangePassword") THEN RETURN coalesce(NEW,OLD); END IF;
  org:=OLD."organizationId";
  PERFORM 1 FROM webdock_auth.organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id WHERE o.id=org FOR UPDATE OF o;
  IF FOUND AND NOT EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=org AND m.id<>OLD.id AND m.role='owner' AND u.role='user' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword") THEN
   RAISE EXCEPTION 'Keep at least one active verified customer owner.' USING ERRCODE='23514';
  END IF;
 ELSE
  IF OLD.role<>'user' OR coalesce(OLD.banned,false) OR NOT OLD."emailVerified" OR OLD."mustChangePassword" THEN RETURN coalesce(NEW,OLD); END IF;
  IF TG_OP='UPDATE' AND NEW.role='user' AND NOT coalesce(NEW.banned,false) AND NEW."emailVerified" AND NOT NEW."mustChangePassword" THEN RETURN NEW; END IF;
  FOR org IN SELECT o.id FROM webdock_auth.organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id JOIN webdock_auth.member m ON m."organizationId"=o.id WHERE m."userId"=OLD.id AND m.role='owner' ORDER BY o.id FOR UPDATE OF o LOOP
   IF NOT EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=org AND m."userId"<>OLD.id AND m.role='owner' AND u.role='user' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword") THEN
    RAISE EXCEPTION 'Keep at least one active verified customer owner.' USING ERRCODE='23514';
   END IF;
  END LOOP;
 END IF;
 RETURN coalesce(NEW,OLD);
END;
$$;
REVOKE ALL ON FUNCTION webdock_auth.guard_customer_owner() FROM PUBLIC;
DROP TRIGGER IF EXISTS webdock_customer_owner_member ON webdock_auth.member;
CREATE TRIGGER webdock_customer_owner_member BEFORE UPDATE OR DELETE ON webdock_auth.member FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_customer_owner();
DROP TRIGGER IF EXISTS webdock_customer_owner_user ON webdock_auth."user";
CREATE TRIGGER webdock_customer_owner_user BEFORE UPDATE OR DELETE ON webdock_auth."user" FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_customer_owner();
`;
