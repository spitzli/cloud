CREATE TABLE webdock_auth.billing_manual_release (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL DEFAULT 'live' CHECK(environment='live'),customer_id varchar NOT NULL,contract_id text NOT NULL UNIQUE,
 actor_id text NOT NULL,reason text NOT NULL CHECK(length(reason) BETWEEN 1 AND 1000),expected_revision integer NOT NULL,created_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(environment,customer_id,contract_id) REFERENCES webdock_auth.billing_contract(environment,customer_id,id) ON DELETE RESTRICT
);
ALTER TABLE webdock_auth.billing_manual_release OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.billing_manual_release ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.billing_manual_release FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.billing_manual_release FROM PUBLIC;
GRANT SELECT,INSERT ON webdock_auth.billing_manual_release TO webdock_auth_runtime;
GRANT SELECT ON webdock_auth.billing_manual_release TO webdock_worker_runtime;
CREATE POLICY billing_owner ON webdock_auth.billing_manual_release TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY billing_release_operator ON webdock_auth.billing_manual_release TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
CREATE POLICY billing_release_worker ON webdock_auth.billing_manual_release FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:live:customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_base_locked(customer text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.customer_allowed(customer,true) THEN RAISE EXCEPTION 'Customer authority required' USING ERRCODE='42501';END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.billing_contract c WHERE c.environment='live' AND c.customer_id=customer AND c.projected_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release r WHERE r.contract_id=c.id));
END $$;
CREATE FUNCTION webdock_auth.guard_paid_base() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE customer text;locked boolean;
BEGIN
 customer:=CASE WHEN TG_OP='DELETE' THEN OLD.customer_id ELSE NEW.customer_id END;
 IF pg_has_role(session_user,'webdock_platform_owner','USAGE') OR EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=session_user AND rolsuper) OR (webdock_rls.worker(ARRAY['polar:live:customer']) AND webdock_rls.request_scope()->>'customerID'=customer) THEN
  IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
 END IF;
 SELECT EXISTS(SELECT 1 FROM webdock_auth.billing_contract c WHERE c.environment='live' AND c.customer_id=customer AND c.projected_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release r WHERE r.contract_id=c.id)) INTO locked;
 IF locked AND (TG_OP<>'UPDATE' OR (OLD.base,OLD.plan_id,OLD.name,OLD.description) IS DISTINCT FROM (NEW.base,NEW.plan_id,NEW.name,NEW.description)) THEN RAISE EXCEPTION 'Paid base requires an explicit manual release' USING ERRCODE='55000';END IF;
 IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.billing_base_locked(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_auth.guard_paid_base() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_base_locked(text),webdock_auth.guard_paid_base() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_base_locked(text) TO webdock_auth_runtime;
CREATE TRIGGER billing_paid_base BEFORE INSERT OR UPDATE OR DELETE ON webdock_auth.tenant_subscription FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_paid_base();
CREATE OR REPLACE FUNCTION webdock_rls.billing_cms_allowed(binding text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE enabled boolean;status text;contract text;
BEGIN
 IF session_user<>'webdock_auth_bootstrap' THEN RAISE EXCEPTION 'Identity service required' USING ERRCODE='42501';END IF;
 SELECT enforcement_enabled INTO enabled FROM webdock_auth.billing_connection WHERE environment='live';IF NOT coalesce(enabled,false) THEN RETURN true;END IF;
 SELECT id,service_status INTO contract,status FROM webdock_auth.billing_contract WHERE environment='live' AND binding_id=binding AND projected_at IS NOT NULL ORDER BY accepted_at DESC,id DESC LIMIT 1;
 RETURN status IS NULL OR status IN ('active','warning') OR EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release WHERE contract_id=contract);
END $$;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;

CREATE OR REPLACE FUNCTION webdock_rls.project_billing_contract(customer text,contract text) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE c webdock_auth.billing_contract; s webdock_auth.billing_subscription; old webdock_auth.tenant_subscription; base jsonb; extra jsonb; revision integer; status text; effective_snapshot jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['polar:live:customer']) OR webdock_rls.request_scope()->>'customerID' IS DISTINCT FROM customer THEN RAISE EXCEPTION 'Live customer billing authority required' USING ERRCODE='42501';END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('plan:'||customer,0));
 SELECT * INTO c FROM webdock_auth.billing_contract WHERE environment='live' AND customer_id=customer AND id=contract FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Contract unavailable' USING ERRCODE='42501';END IF;
 IF EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release WHERE contract_id=c.id) THEN RETURN 'manual';END IF;
 SELECT * INTO s FROM webdock_auth.billing_subscription WHERE environment='live' AND customer_id=customer AND contract_id=contract;
 IF NOT FOUND THEN RETURN 'pending';END IF;
 IF c.state IN ('withdrawn','paid_needs_review') OR (c.state='ended' AND s.status='active') THEN
  IF s.initial_order_paid THEN UPDATE webdock_auth.billing_contract SET state='paid_needs_review' WHERE id=c.id;END IF;RETURN 'needs_review';
 END IF;
 IF c.projected_at IS NOT NULL AND s.status IN ('canceled','unpaid','paused') THEN
  status:=CASE WHEN s.status='paused' THEN 'suspended' ELSE 'ended' END;
  UPDATE webdock_auth.billing_contract SET state=status,service_status=status,ended_at=CASE WHEN status='ended' THEN coalesce(ended_at,now()) END WHERE id=c.id;RETURN status;
 END IF;
 IF c.projected_at IS NOT NULL AND s.status='past_due' THEN
  UPDATE webdock_auth.billing_contract SET service_status='warning' WHERE id=c.id;RETURN 'warning';
 END IF;
 IF s.status<>'active' OR NOT s.initial_order_paid THEN RETURN 'pending';END IF;
 SELECT * INTO old FROM webdock_auth.tenant_subscription WHERE customer_id=customer FOR UPDATE;
 IF c.projected_at IS NULL AND old.base IS DISTINCT FROM c.previous_base->'base' THEN UPDATE webdock_auth.billing_contract SET state='paid_needs_review' WHERE id=c.id;RETURN 'needs_review';END IF;
 SELECT snapshot INTO effective_snapshot FROM webdock_auth.billing_change WHERE environment='live' AND contract_id=c.id AND billing_change.status='applied' ORDER BY applied_at DESC,id DESC LIMIT 1;
 effective_snapshot:=coalesce(effective_snapshot,c.snapshot);
 base:=jsonb_build_object('storageBytes',(effective_snapshot->'entitlements'->>'mediaBytes')::bigint,'mailMessages',(effective_snapshot->'entitlements'->>'mailMonthly')::bigint,'transferBytes',0,'websites',(effective_snapshot->'entitlements'->>'websites')::integer,'editors',(effective_snapshot->'entitlements'->>'editors')::integer);
 IF base->>'websites' IS DISTINCT FROM '1' OR (base->>'storageBytes')::bigint<1 OR (base->>'storageBytes')::bigint>9007199254740991 OR (base->>'editors')::integer<1 THEN RAISE EXCEPTION 'Invalid purchased allowances' USING ERRCODE='22023';END IF;
 extra:=coalesce(old.extras,jsonb_build_object('storageBytes',0,'mailMessages',0,'transferBytes',0,'websites',0,'editors',0));revision:=coalesce(old.revision,0);
 IF old.base IS DISTINCT FROM base OR old.name IS DISTINCT FROM effective_snapshot->>'name' OR old.description IS DISTINCT FROM effective_snapshot->>'description' OR old.plan_id IS NOT NULL THEN
  INSERT INTO webdock_auth.tenant_subscription(customer_id,plan_id,name,description,base,extras,revision,updated_by) VALUES(customer,NULL,effective_snapshot->>'name',effective_snapshot->>'description',base,extra,revision+1,'system:polar:live')
  ON CONFLICT(customer_id) DO UPDATE SET plan_id=NULL,name=EXCLUDED.name,description=EXCLUDED.description,base=EXCLUDED.base,extras=EXCLUDED.extras,revision=EXCLUDED.revision,updated_by=EXCLUDED.updated_by,updated_at=now();
  INSERT INTO webdock_auth.subscription_history(customer_id,revision,name,description,base,extras,plan_id,actor_id) SELECT customer_id,tenant_subscription.revision,name,description,tenant_subscription.base,extras,plan_id,updated_by FROM webdock_auth.tenant_subscription WHERE customer_id=customer;
 END IF;
 UPDATE webdock_auth.billing_contract SET state='active',service_status='active',projected_at=coalesce(projected_at,now()),projected_hash=md5(base::text),ended_at=NULL WHERE id=c.id;
 RETURN 'active';
END $$;
