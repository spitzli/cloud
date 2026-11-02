GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_customer_lookup(env text,external text,polar uuid) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE customer text;
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.worker(ARRAY['polar:'||env||':dispatcher']) THEN RAISE EXCEPTION 'Billing dispatcher required' USING ERRCODE='42501';END IF;
 SELECT customer_id INTO customer FROM webdock_auth.billing_customer WHERE environment=env AND external_id=external AND (polar_customer_id IS NULL OR polar_customer_id=polar) AND (env='live' OR is_test);
 RETURN customer;
END $$;
CREATE FUNCTION webdock_rls.billing_due_customers(env text) RETURNS TABLE(customer_id text,interval_minutes integer) LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.worker(ARRAY['polar:'||env||':dispatcher']) THEN RAISE EXCEPTION 'Billing dispatcher required' USING ERRCODE='42501';END IF;
 RETURN QUERY SELECT c.customer_id::text,CASE WHEN EXISTS(SELECT 1 FROM webdock_auth.billing_contract b WHERE b.environment=env AND b.customer_id=c.customer_id AND b.state IN ('pending','needs_review','paid_needs_review')) OR EXISTS(SELECT 1 FROM webdock_auth.billing_subscription s WHERE s.environment=env AND s.customer_id=c.customer_id AND s.status IN ('past_due','incomplete')) THEN 15 ELSE 360 END
 FROM webdock_auth.billing_customer c WHERE c.environment=env AND (env='live' OR c.is_test) AND EXISTS(SELECT 1 FROM webdock_auth.billing_contract b WHERE b.environment=env AND b.customer_id=c.customer_id AND b.state<>'ended') ORDER BY c.reconciled_at NULLS FIRST,c.customer_id LIMIT 1000;
END $$;
CREATE FUNCTION webdock_rls.billing_catalog_terms(env text,polar uuid DEFAULT NULL) RETURNS SETOF jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.worker(ARRAY['polar:'||env||':catalog']) THEN RAISE EXCEPTION 'Billing catalog worker required' USING ERRCODE='42501';END IF;
 RETURN QUERY SELECT jsonb_build_object('organizationID',c.organization_id,'versionID',v.id,'operationID',m.operation_id,'sku',p.sku,'hash',m.desired_hash,'name',p.name,'description',v.description,'amount',v.amount,'currency',v.currency,'taxBehavior',v.tax_behavior,'interval',v.interval,'visibility',p.visibility,'productID',m.polar_product_id)
 FROM webdock_auth.billing_product_mapping m JOIN webdock_auth.billing_product_version v ON v.id=m.version_id JOIN webdock_auth.billing_product p ON p.id=v.product_id JOIN webdock_auth.billing_connection c ON c.environment=m.environment
 WHERE m.environment=env AND m.polar_product_id IS NOT NULL AND (polar IS NULL OR m.polar_product_id=polar) AND m.status<>'archived' ORDER BY m.version_id;
END $$;
ALTER FUNCTION webdock_rls.billing_customer_lookup(text,text,uuid) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_due_customers(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_catalog_terms(text,uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_customer_lookup(text,text,uuid),webdock_rls.billing_due_customers(text),webdock_rls.billing_catalog_terms(text,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_customer_lookup(text,text,uuid),webdock_rls.billing_due_customers(text),webdock_rls.billing_catalog_terms(text,uuid) TO webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
