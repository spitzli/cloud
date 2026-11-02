DROP INDEX webdock_auth.billing_one_website_base;
CREATE UNIQUE INDEX billing_one_website_base ON webdock_auth.billing_contract(environment,customer_id,service_key) WHERE state NOT IN ('ended','paid_needs_review');
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.project_billing_contract(customer text,contract text) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE c webdock_auth.billing_contract; s webdock_auth.billing_subscription; old webdock_auth.tenant_subscription; base jsonb; extra jsonb; revision integer; status text;
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
 base:=jsonb_build_object('storageBytes',(c.snapshot->'entitlements'->>'mediaBytes')::bigint,'mailMessages',(c.snapshot->'entitlements'->>'mailMonthly')::bigint,'transferBytes',0,'websites',(c.snapshot->'entitlements'->>'websites')::integer,'editors',(c.snapshot->'entitlements'->>'editors')::integer);
 IF base->>'websites' IS DISTINCT FROM '1' OR (base->>'storageBytes')::bigint<1 OR (base->>'storageBytes')::bigint>9007199254740991 OR (base->>'editors')::integer<1 THEN RAISE EXCEPTION 'Invalid purchased allowances' USING ERRCODE='22023';END IF;
 extra:=coalesce(old.extras,jsonb_build_object('storageBytes',0,'mailMessages',0,'transferBytes',0,'websites',0,'editors',0));revision:=coalesce(old.revision,0);
 IF old.base IS DISTINCT FROM base OR old.name IS DISTINCT FROM c.snapshot->>'name' OR old.description IS DISTINCT FROM c.snapshot->>'description' OR old.plan_id IS NOT NULL THEN
  INSERT INTO webdock_auth.tenant_subscription(customer_id,plan_id,name,description,base,extras,revision,updated_by) VALUES(customer,NULL,c.snapshot->>'name',c.snapshot->>'description',base,extra,revision+1,'system:polar:live')
  ON CONFLICT(customer_id) DO UPDATE SET plan_id=NULL,name=EXCLUDED.name,description=EXCLUDED.description,base=EXCLUDED.base,extras=EXCLUDED.extras,revision=EXCLUDED.revision,updated_by=EXCLUDED.updated_by,updated_at=now();
  INSERT INTO webdock_auth.subscription_history(customer_id,revision,name,description,base,extras,plan_id,actor_id) SELECT customer_id,tenant_subscription.revision,name,description,tenant_subscription.base,extras,plan_id,updated_by FROM webdock_auth.tenant_subscription WHERE customer_id=customer;
 END IF;
 UPDATE webdock_auth.billing_contract SET state='active',service_status='active',projected_at=coalesce(projected_at,now()),projected_hash=md5(base::text),ended_at=NULL WHERE id=c.id;
 RETURN 'active';
END $$;
CREATE FUNCTION webdock_rls.billing_cms_allowed(binding text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE enabled boolean;status text;
BEGIN
 IF session_user<>'webdock_auth_bootstrap' THEN RAISE EXCEPTION 'Identity service required' USING ERRCODE='42501';END IF;
 SELECT enforcement_enabled INTO enabled FROM webdock_auth.billing_connection WHERE environment='live';IF NOT coalesce(enabled,false) THEN RETURN true;END IF;
 SELECT service_status INTO status FROM webdock_auth.billing_contract WHERE environment='live' AND binding_id=binding AND projected_at IS NOT NULL ORDER BY accepted_at DESC,id DESC LIMIT 1;
 RETURN status IS NULL OR status IN ('active','warning');
END $$;
ALTER FUNCTION webdock_rls.project_billing_contract(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_cms_allowed(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_billing_contract(text,text),webdock_rls.billing_cms_allowed(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_billing_contract(text,text) TO webdock_worker_runtime;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_cms_allowed(text) TO webdock_auth_bootstrap;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
