-- Qualify the function parameter against the existing numeric version column.
CREATE OR REPLACE FUNCTION webdock_rls.billing_version(env text,version text,customer text,offer text DEFAULT NULL) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 IF env='sandbox' AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_customer WHERE environment=env AND customer_id=customer AND is_test) THEN RAISE EXCEPTION 'Explicit sandbox customer required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('versionID',v.id,'productID',m.polar_product_id,'priceIDs',m.price_ids,'name',p.name,'description',v.description,'terms',v.terms_text,'amount',v.amount::text,'currency',v.currency,'taxBehavior',v.tax_behavior,'interval',v.interval,'entitlements',v.entitlements,'termsHash',v.terms_hash) INTO result
 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id JOIN webdock_auth.billing_product_mapping m ON m.version_id=v.id AND m.environment=env
 WHERE v.id=billing_version.version AND v.frozen AND v.terms_text<>'' AND m.status='ready' AND m.polar_product_id IS NOT NULL AND p.operationally_ready AND NOT p.archived AND p.family='website'
 AND (p.visibility='public' OR EXISTS(SELECT 1 FROM webdock_auth.billing_offer o WHERE o.id=offer AND o.environment=env AND o.customer_id=customer AND o.version_id=v.id));
 RETURN result;
END $$;
