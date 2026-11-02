-- Native0041: outbox work retains the exact authority admitted to the invitation.
-- Legacy rows deliberately remain unbound and are canceled at dispatch; explicitly resend them.
ALTER TABLE webdock_auth.access_delivery_job
 ADD COLUMN admitted_customer_scope text CHECK(admitted_customer_scope IS NULL OR admitted_customer_scope ~ '^[a-f0-9]{32}$'),
 ADD COLUMN admission_bound boolean NOT NULL DEFAULT false;

ALTER POLICY scoped_access ON webdock_auth.access_delivery_job
 WITH CHECK(actor_id=webdock_rls.subject()
  AND actor_session_id=webdock_rls.actor()->>'sessionID'
  AND admission_bound
  AND admitted_customer_scope IS NOT DISTINCT FROM webdock_rls.customer_scope()
  AND webdock_rls.organization_allowed(organization_id,true));

CREATE FUNCTION webdock_auth.guard_access_delivery_admission() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN
 IF session_user='webdock_auth_runtime' AND
 ROW(NEW.id,NEW.invitation_id,NEW.organization_id,NEW.user_id,NEW.binding_id,
     NEW.actor_id,NEW.actor_session_id,NEW.generation,NEW.kind,NEW.payload,
     NEW.admitted_customer_scope,NEW.admission_bound)
 IS DISTINCT FROM
 ROW(OLD.id,OLD.invitation_id,OLD.organization_id,OLD.user_id,OLD.binding_id,
     OLD.actor_id,OLD.actor_session_id,OLD.generation,OLD.kind,OLD.payload,
     OLD.admitted_customer_scope,OLD.admission_bound)
 THEN RAISE EXCEPTION 'Invitation delivery admission is immutable' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_access_delivery_admission() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_access_delivery_admission() FROM PUBLIC;
CREATE TRIGGER guard_access_delivery_admission BEFORE UPDATE ON webdock_auth.access_delivery_job
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_access_delivery_admission();
