-- Preserve the existing extras-only UPSERT while fencing replacement of a paid base.
CREATE OR REPLACE FUNCTION webdock_auth.guard_paid_base() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE customer text;locked boolean; previous webdock_auth.tenant_subscription;
BEGIN
 customer:=CASE WHEN TG_OP='DELETE' THEN OLD.customer_id ELSE NEW.customer_id END;
 IF pg_has_role(session_user,'webdock_platform_owner','USAGE') OR EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=session_user AND rolsuper) OR (webdock_rls.worker(ARRAY['polar:live:customer']) AND webdock_rls.request_scope()->>'customerID'=customer) THEN
  IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
 END IF;
 IF TG_OP='UPDATE' AND NEW.customer_id IS DISTINCT FROM OLD.customer_id THEN RAISE EXCEPTION 'Subscription customer cannot change' USING ERRCODE='55000';END IF;
 IF TG_OP='INSERT' THEN SELECT * INTO previous FROM webdock_auth.tenant_subscription WHERE customer_id=customer;END IF;
 SELECT EXISTS(SELECT 1 FROM webdock_auth.billing_contract c WHERE c.environment='live' AND c.customer_id=customer AND c.projected_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release r WHERE r.contract_id=c.id)) INTO locked;
 IF locked AND (TG_OP='DELETE' OR (TG_OP='INSERT' AND (previous.base,previous.plan_id,previous.name,previous.description) IS DISTINCT FROM (NEW.base,NEW.plan_id,NEW.name,NEW.description)) OR (TG_OP='UPDATE' AND (OLD.base,OLD.plan_id,OLD.name,OLD.description) IS DISTINCT FROM (NEW.base,NEW.plan_id,NEW.name,NEW.description))) THEN RAISE EXCEPTION 'Paid base requires an explicit manual release' USING ERRCODE='55000';END IF;
 IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
END $$;
