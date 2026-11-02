CREATE TABLE webdock_auth.billing_change (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL,customer_id varchar NOT NULL,contract_id text NOT NULL,subscription_id text NOT NULL,version_id text NOT NULL,
 operation_id uuid NOT NULL,from_product_id uuid NOT NULL,direction text NOT NULL CHECK(direction IN ('upgrade','downgrade')),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object'),terms_hash text NOT NULL CHECK(terms_hash ~ '^[a-f0-9]{64}$'),offer_id text,
 period_end timestamptz NOT NULL,status text NOT NULL DEFAULT 'reserved' CHECK(status IN ('reserved','changing','scheduled','awaiting_payment','applied','failed','needs_review')),
 baseline_order_ids jsonb NOT NULL DEFAULT '[]',claim_token uuid,lease_until timestamptz,effective_at timestamptz,error_code text,
 accepted_by text NOT NULL,accepted_at timestamptz NOT NULL DEFAULT now(),applied_at timestamptz,
 FOREIGN KEY(environment,customer_id,contract_id,subscription_id) REFERENCES webdock_auth.billing_subscription(environment,customer_id,contract_id,id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,version_id) REFERENCES webdock_auth.billing_product_mapping(environment,version_id) ON DELETE RESTRICT,
 UNIQUE(environment,operation_id)
);
CREATE UNIQUE INDEX billing_one_pending_change ON webdock_auth.billing_change(environment,contract_id) WHERE status NOT IN ('applied','failed');
ALTER TABLE webdock_auth.billing_change OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.billing_change ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.billing_change FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.billing_change FROM PUBLIC;
CREATE POLICY billing_owner ON webdock_auth.billing_change TO webdock_platform_owner USING(true) WITH CHECK(true);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_change TO webdock_auth_runtime,webdock_worker_runtime;
CREATE POLICY billing_change_read ON webdock_auth.billing_change FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR webdock_rls.billing_admin(customer_id));
CREATE POLICY billing_change_insert ON webdock_auth.billing_change FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.billing_admin(customer_id));
CREATE POLICY billing_change_lock ON webdock_auth.billing_change FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR webdock_rls.billing_admin(customer_id)) WITH CHECK(false);
CREATE POLICY billing_change_worker ON webdock_auth.billing_change TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE FUNCTION webdock_auth.billing_change_immutable() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF (to_jsonb(NEW)-ARRAY['status','baseline_order_ids','claim_token','lease_until','effective_at','error_code','applied_at']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['status','baseline_order_ids','claim_token','lease_until','effective_at','error_code','applied_at']) THEN RAISE EXCEPTION 'Accepted change conditions are immutable' USING ERRCODE='55000';END IF;
 IF OLD.status<>'reserved' AND NEW.baseline_order_ids IS DISTINCT FROM OLD.baseline_order_ids THEN RAISE EXCEPTION 'Change baseline is immutable' USING ERRCODE='55000';END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.billing_change_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.billing_change_immutable() FROM PUBLIC;
CREATE TRIGGER billing_change_immutable BEFORE UPDATE ON webdock_auth.billing_change FOR EACH ROW EXECUTE FUNCTION webdock_auth.billing_change_immutable();

CREATE OR REPLACE FUNCTION webdock_rls.project_billing_contract(customer text,contract text) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE c webdock_auth.billing_contract; s webdock_auth.billing_subscription; old webdock_auth.tenant_subscription; base jsonb; extra jsonb; revision integer; status text; effective_snapshot jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['polar:live:customer']) OR webdock_rls.request_scope()->>'customerID' IS DISTINCT FROM customer THEN RAISE EXCEPTION 'Live customer billing authority required' USING ERRCODE='42501';END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('plan:'||customer,0));
 SELECT * INTO c FROM webdock_auth.billing_contract WHERE environment='live' AND customer_id=customer AND id=contract FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Contract unavailable' USING ERRCODE='42501';END IF;
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
