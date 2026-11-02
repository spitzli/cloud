ALTER TABLE webdock_auth.billing_product_version ADD COLUMN terms_text text NOT NULL DEFAULT '';
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_admin(customer text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT coalesce(webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb AND EXISTS(
 SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id JOIN webdock_auth.member m ON m."organizationId"=t.organization_id
 WHERE t.customer_id=customer AND c.status='active' AND m."userId"=webdock_rls.subject() AND m.role IN ('owner','admin')),false)
$$;
CREATE FUNCTION webdock_rls.billing_sale_config(env text,customer text) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 IF env='sandbox' AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_customer WHERE environment=env AND customer_id=customer AND is_test) THEN RAISE EXCEPTION 'Explicit sandbox customer required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('organizationID',organization_id,'salesEnabled',sales_enabled,'endpointID',endpoint_id) INTO result FROM webdock_auth.billing_connection WHERE environment=env;
 RETURN result;
END $$;
CREATE FUNCTION webdock_rls.billing_site(customer text,project text,binding text,management_url text) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('organizationID',b.organization_id,'bindingID',b.id,'projectID',p.id,'name',p.name) INTO result
 FROM webdock_auth.app_binding b JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id JOIN webdock_admin.projects p ON p.customer_id=t.customer_id JOIN webdock_admin.cms_instances i ON i.project_id=p.id
 WHERE t.customer_id=customer AND p.id=project AND b.id=binding AND b.enabled AND p.status='active' AND i.status='active' AND i.admin_u_r_l=management_url
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion WHERE project_id=p.id);
 RETURN result;
END $$;
CREATE TABLE webdock_auth.billing_offer (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL CHECK(environment IN ('sandbox','live')),
 customer_id varchar NOT NULL,version_id text NOT NULL,token_hash text NOT NULL UNIQUE,
 expected_revision integer NOT NULL CHECK(expected_revision>=0),expires_at timestamptz NOT NULL,
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','revoked')),
 created_by text NOT NULL,created_at timestamptz NOT NULL DEFAULT now(),accepted_by text,accepted_at timestamptz,contract_id text,
 FOREIGN KEY(environment,customer_id) REFERENCES webdock_auth.billing_customer(environment,customer_id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,version_id) REFERENCES webdock_auth.billing_product_mapping(environment,version_id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,customer_id,contract_id) REFERENCES webdock_auth.billing_contract(environment,customer_id,id) ON DELETE RESTRICT,
 UNIQUE(environment,version_id)
);
ALTER TABLE webdock_auth.billing_offer OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.billing_offer ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.billing_offer FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.billing_offer FROM PUBLIC;
CREATE POLICY billing_owner ON webdock_auth.billing_offer TO webdock_platform_owner USING(true) WITH CHECK(true);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_offer TO webdock_auth_runtime;
CREATE POLICY billing_offer_read ON webdock_auth.billing_offer FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR webdock_rls.billing_admin(customer_id));
CREATE POLICY billing_offer_write ON webdock_auth.billing_offer FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY billing_offer_update ON webdock_auth.billing_offer FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR webdock_rls.billing_admin(customer_id)) WITH CHECK(webdock_rls.operator() OR webdock_rls.billing_admin(customer_id));
CREATE FUNCTION webdock_auth.billing_offer_immutable() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF (to_jsonb(NEW)-ARRAY['status','accepted_by','accepted_at','contract_id']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['status','accepted_by','accepted_at','contract_id']) THEN RAISE EXCEPTION 'Offer conditions are immutable' USING ERRCODE='55000';END IF;
 IF OLD.status<>'pending' AND NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'Offer already finalized' USING ERRCODE='55000';END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.billing_offer_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.billing_offer_immutable() FROM PUBLIC;
CREATE TRIGGER billing_offer_immutable BEFORE UPDATE ON webdock_auth.billing_offer FOR EACH ROW EXECUTE FUNCTION webdock_auth.billing_offer_immutable();
CREATE FUNCTION webdock_rls.billing_version(env text,version text,customer text,offer text DEFAULT NULL) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF env NOT IN ('sandbox','live') OR NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 IF env='sandbox' AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_customer WHERE environment=env AND customer_id=customer AND is_test) THEN RAISE EXCEPTION 'Explicit sandbox customer required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('versionID',v.id,'productID',m.polar_product_id,'priceIDs',m.price_ids,'name',p.name,'description',v.description,'terms',v.terms_text,'amount',v.amount::text,'currency',v.currency,'taxBehavior',v.tax_behavior,'interval',v.interval,'entitlements',v.entitlements,'termsHash',v.terms_hash) INTO result
 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id JOIN webdock_auth.billing_product_mapping m ON m.version_id=v.id AND m.environment=env
 WHERE v.id=version AND v.frozen AND v.terms_text<>'' AND m.status='ready' AND m.polar_product_id IS NOT NULL AND p.operationally_ready AND NOT p.archived AND p.family='website'
 AND (p.visibility='public' OR EXISTS(SELECT 1 FROM webdock_auth.billing_offer o WHERE o.id=offer AND o.environment=env AND o.customer_id=customer AND o.version_id=v.id));
 RETURN result;
END $$;
-- Paginated scheduling; failed older customers cannot starve later customers.
DROP FUNCTION webdock_rls.billing_due_customers(text);
CREATE FUNCTION webdock_rls.billing_due_customers(env text,after_customer text DEFAULT '',page_limit integer DEFAULT 100) RETURNS TABLE(customer_id text,interval_minutes integer) LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF env NOT IN ('sandbox','live') OR page_limit<1 OR page_limit>100 OR NOT webdock_rls.worker(ARRAY['polar:'||env||':dispatcher']) THEN RAISE EXCEPTION 'Billing dispatcher required' USING ERRCODE='42501';END IF;
 RETURN QUERY SELECT c.customer_id::text,CASE WHEN EXISTS(SELECT 1 FROM webdock_auth.billing_contract b WHERE b.environment=env AND b.customer_id=c.customer_id AND b.state IN ('pending','needs_review','paid_needs_review')) OR EXISTS(SELECT 1 FROM webdock_auth.billing_subscription s WHERE s.environment=env AND s.customer_id=c.customer_id AND s.status IN ('past_due','incomplete')) THEN 15 ELSE 360 END
 FROM webdock_auth.billing_customer c WHERE c.environment=env AND c.customer_id::text>after_customer AND (env='live' OR c.is_test) AND EXISTS(SELECT 1 FROM webdock_auth.billing_contract b WHERE b.environment=env AND b.customer_id=c.customer_id AND b.state<>'ended') ORDER BY c.customer_id::text LIMIT page_limit;
END $$;
ALTER FUNCTION webdock_rls.billing_admin(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_sale_config(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_site(text,text,text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_version(text,text,text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.billing_due_customers(text,text,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_admin(text),webdock_rls.billing_sale_config(text,text),webdock_rls.billing_site(text,text,text,text),webdock_rls.billing_version(text,text,text,text),webdock_rls.billing_due_customers(text,text,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_admin(text),webdock_rls.billing_sale_config(text,text),webdock_rls.billing_site(text,text,text,text),webdock_rls.billing_version(text,text,text,text) TO webdock_auth_runtime;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_due_customers(text,text,integer) TO webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
