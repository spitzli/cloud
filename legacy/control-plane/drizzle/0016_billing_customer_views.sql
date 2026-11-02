-- Qualify the function parameter against the existing numeric version column.
CREATE OR REPLACE FUNCTION webdock_rls.billing_version(env text,version text,customer text,offer text DEFAULT NULL) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 IF env='sandbox' AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_customer WHERE environment=env AND customer_id=customer AND is_test) THEN RAISE EXCEPTION 'Explicit sandbox customer required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('versionID',v.id,'productID',m.polar_product_id,'priceIDs',m.price_ids,'name',p.name,'description',v.description,'terms',v.terms_text,'amount',v.amount::text,'currency',v.currency,'taxBehavior',v.tax_behavior,'interval',v.interval,'entitlements',v.entitlements,'termsHash',v.terms_hash) INTO result
 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id JOIN webdock_auth.billing_product_mapping m ON m.version_id=v.id AND m.environment=env
 WHERE v.id=billing_version.version AND v.frozen AND v.terms_text<>'' AND m.status='ready' AND m.polar_product_id IS NOT NULL AND p.operationally_ready AND NOT p.archived AND p.family='website'
 AND (p.visibility<>'public' OR v.version=(SELECT max(newest.version) FROM webdock_auth.billing_product_version newest JOIN webdock_auth.billing_product_mapping published ON published.version_id=newest.id AND published.environment=env AND published.status='ready' WHERE newest.product_id=p.id))
 AND (p.visibility='public' OR EXISTS(SELECT 1 FROM webdock_auth.billing_offer o WHERE o.id=offer AND o.environment=env AND o.customer_id=customer AND o.version_id=v.id));
 RETURN result;
END $$;

GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_catalog(env text,customer text) RETURNS SETOF jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 RETURN QUERY SELECT value FROM (SELECT webdock_rls.billing_version(env,v.id,customer,NULL) AS value FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id WHERE p.visibility='public') available WHERE value IS NOT NULL;
END $$;
CREATE FUNCTION webdock_rls.billing_sites(customer text,studio text) RETURNS SETOF jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 RETURN QUERY SELECT jsonb_build_object('bindingID',b.id,'projectID',p.id,'name',p.name) FROM webdock_auth.app_binding b
 JOIN webdock_auth."oauthClient" client ON client."clientId"=b.client_id AND NOT coalesce(client.disabled,false)
 JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id JOIN webdock_admin.projects p ON p.customer_id=t.customer_id JOIN webdock_admin.cms_instances i ON i.project_id=p.id
 WHERE t.customer_id=customer AND b.enabled AND p.status='active' AND i.status='active' AND i.admin_u_r_l=studio||'/sites/'||b.id AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion WHERE project_id=p.id) ORDER BY p.name,p.id;
END $$;
ALTER FUNCTION webdock_rls.billing_catalog(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_sites(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_catalog(text,text),webdock_rls.billing_sites(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_catalog(text,text),webdock_rls.billing_sites(text,text) TO webdock_auth_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
