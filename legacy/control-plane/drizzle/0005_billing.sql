-- Additive only. No catalog seed, customer migration, sales or enforcement enablement.
CREATE TABLE webdock_auth.billing_connection (
 environment text PRIMARY KEY CHECK(environment IN ('sandbox','live')),
 organization_id uuid, organization_name text, locked boolean NOT NULL DEFAULT false,
 encrypted_token text, token_suffix text, token_version integer NOT NULL DEFAULT 0,
 token_expires_at timestamptz, encrypted_webhook_secret text, webhook_version integer NOT NULL DEFAULT 0,
 previous_webhook_secret text, previous_webhook_version integer, previous_webhook_until timestamptz,
 endpoint_id uuid, api_version text NOT NULL DEFAULT '2026-10' CHECK(api_version='2026-10'),
 sales_enabled boolean NOT NULL DEFAULT false, catalog_publish_enabled boolean NOT NULL DEFAULT false,
 enforcement_enabled boolean NOT NULL DEFAULT false CHECK(environment='live' OR NOT enforcement_enabled),
 revision bigint NOT NULL DEFAULT 0, checked_at timestamptz, received_at timestamptz, reconciled_at timestamptz,
 updated_by text, updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.billing_product (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), sku text NOT NULL UNIQUE,
 name text NOT NULL, family text NOT NULL CHECK(family IN ('website','media','editors','mail','apps','connect','database')),
 visibility text NOT NULL DEFAULT 'internal' CHECK(visibility IN ('public','private','internal')),
 operationally_ready boolean NOT NULL DEFAULT false, archived boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.billing_product_version (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), product_id text NOT NULL REFERENCES webdock_auth.billing_product(id) ON DELETE RESTRICT,
 version integer NOT NULL CHECK(version>0), amount bigint NOT NULL CHECK(amount>0 AND amount<=9007199254740991),
 currency text NOT NULL CHECK(currency IN ('EUR','CHF')), tax_behavior text NOT NULL CHECK(tax_behavior IN ('inclusive','exclusive')),
 interval text NOT NULL CHECK(interval IN ('month','year')), description text NOT NULL,
 entitlements jsonb NOT NULL CHECK(jsonb_typeof(entitlements)='object'), terms_hash text NOT NULL CHECK(terms_hash ~ '^[a-f0-9]{64}$'),
 frozen boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now(), UNIQUE(product_id,version)
);
CREATE TABLE webdock_auth.billing_product_mapping (
 environment text NOT NULL CHECK(environment IN ('sandbox','live')), version_id text NOT NULL REFERENCES webdock_auth.billing_product_version(id) ON DELETE RESTRICT,
 polar_product_id uuid, price_ids jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(price_ids)='array'),
 status text NOT NULL DEFAULT 'draft' CHECK(status IN ('draft','publishing','ready','archived','failed','needs_review')),
 operation_id uuid, desired_hash text, checked_at timestamptz, error_code text,
 PRIMARY KEY(environment,version_id), UNIQUE(environment,polar_product_id), UNIQUE(environment,operation_id)
);
CREATE TABLE webdock_auth.billing_customer (
 environment text NOT NULL CHECK(environment IN ('sandbox','live')), customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE RESTRICT,
 polar_customer_id uuid, external_id text GENERATED ALWAYS AS ('webdock:'||customer_id) STORED,
 billing_email text, is_test boolean NOT NULL DEFAULT false CHECK(environment<>'sandbox' OR is_test),
 generation bigint NOT NULL DEFAULT 0, reconciled_at timestamptz,
 PRIMARY KEY(environment,customer_id), UNIQUE(environment,polar_customer_id), UNIQUE(environment,external_id)
);
CREATE UNIQUE INDEX billing_binding_customer_org ON webdock_auth.tenant_customer(organization_id,customer_id);
CREATE UNIQUE INDEX billing_binding_org ON webdock_auth.app_binding(id,organization_id);
CREATE UNIQUE INDEX billing_project_customer ON webdock_admin.projects(id,customer_id);
CREATE TABLE webdock_auth.billing_contract (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), environment text NOT NULL CHECK(environment IN ('sandbox','live')),
 customer_id varchar NOT NULL, version_id text NOT NULL, organization_id text NOT NULL, binding_id text NOT NULL, project_id varchar,
 service_key text NOT NULL DEFAULT 'website-base' CHECK(service_key='website-base'),
 mode text NOT NULL DEFAULT 'polar' CHECK(mode IN ('polar','manual','sponsored')),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object'), previous_base jsonb,
 terms_hash text NOT NULL CHECK(terms_hash ~ '^[a-f0-9]{64}$'), offer_id text,
 accepted_by text NOT NULL, accepted_at timestamptz NOT NULL DEFAULT now(),
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','active','suspended','ended','withdrawn','needs_review','paid_needs_review')),
 service_status text NOT NULL DEFAULT 'pending' CHECK(service_status IN ('pending','active','warning','suspended','ended')),
 ended_at timestamptz, projected_hash text, projected_at timestamptz,
 FOREIGN KEY(environment,customer_id) REFERENCES webdock_auth.billing_customer(environment,customer_id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,version_id) REFERENCES webdock_auth.billing_product_mapping(environment,version_id) ON DELETE RESTRICT,
 FOREIGN KEY(organization_id,customer_id) REFERENCES webdock_auth.tenant_customer(organization_id,customer_id) ON DELETE RESTRICT,
 FOREIGN KEY(binding_id,organization_id) REFERENCES webdock_auth.app_binding(id,organization_id) ON DELETE RESTRICT,
 FOREIGN KEY(project_id,customer_id) REFERENCES webdock_admin.projects(id,customer_id) ON DELETE RESTRICT,
 UNIQUE(environment,customer_id,id)
);
CREATE UNIQUE INDEX billing_one_website_base ON webdock_auth.billing_contract(environment,customer_id,service_key) WHERE state NOT IN ('ended','withdrawn');
CREATE TABLE webdock_auth.billing_checkout (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), environment text NOT NULL,customer_id varchar NOT NULL,contract_id text NOT NULL,
 operation_id uuid NOT NULL, polar_checkout_id uuid, expected_product_id uuid NOT NULL, expected_customer_id uuid,
 amount bigint NOT NULL CHECK(amount>0 AND amount<=9007199254740991),currency text NOT NULL CHECK(currency IN ('EUR','CHF')),
 encrypted_url text, expires_at timestamptz,
 state text NOT NULL DEFAULT 'reserved' CHECK(state IN ('reserved','creating','open','paid','expired','needs_review','paid_needs_review','failed')),
 created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(environment,customer_id,contract_id) REFERENCES webdock_auth.billing_contract(environment,customer_id,id) ON DELETE RESTRICT,
 UNIQUE(environment,operation_id),UNIQUE(environment,polar_checkout_id),UNIQUE(environment,contract_id),UNIQUE(environment,customer_id,id)
);
CREATE TABLE webdock_auth.billing_subscription (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL,customer_id varchar NOT NULL,contract_id text NOT NULL,
 polar_subscription_id uuid NOT NULL,polar_product_id uuid NOT NULL,checkout_id text NOT NULL,
 status text NOT NULL,amount bigint CHECK(amount>=0 AND amount<=9007199254740991),currency text CHECK(currency IN ('EUR','CHF')),
 period_start timestamptz,period_end timestamptz,cancel_at_period_end boolean NOT NULL DEFAULT false,canceled_at timestamptz,pending_update jsonb,
 past_due_at timestamptz,reconciled_at timestamptz,generation bigint NOT NULL DEFAULT 0,initial_order_paid boolean NOT NULL DEFAULT false,
 FOREIGN KEY(environment,customer_id,contract_id) REFERENCES webdock_auth.billing_contract(environment,customer_id,id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,customer_id,checkout_id) REFERENCES webdock_auth.billing_checkout(environment,customer_id,id) ON DELETE RESTRICT,
 UNIQUE(environment,polar_subscription_id),UNIQUE(environment,contract_id),UNIQUE(environment,customer_id,id)
);
CREATE TABLE webdock_auth.billing_order (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL,customer_id varchar NOT NULL,contract_id text NOT NULL,
 polar_order_id uuid NOT NULL,subscription_id text NOT NULL,status text NOT NULL,
 net_amount bigint NOT NULL CHECK(net_amount>=0),tax_amount bigint NOT NULL CHECK(tax_amount>=0),total_amount bigint NOT NULL CHECK(total_amount=net_amount+tax_amount),
 currency text NOT NULL CHECK(currency IN ('EUR','CHF')),refunded_amount bigint NOT NULL DEFAULT 0 CHECK(refunded_amount>=0 AND refunded_amount<=total_amount),
 paid_at timestamptz,reconciled_at timestamptz,
 FOREIGN KEY(environment,customer_id,contract_id) REFERENCES webdock_auth.billing_contract(environment,customer_id,id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,customer_id,subscription_id) REFERENCES webdock_auth.billing_subscription(environment,customer_id,id) ON DELETE RESTRICT,
 UNIQUE(environment,polar_order_id)
);
CREATE TABLE webdock_auth.billing_webhook_event (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL CHECK(environment IN ('sandbox','live')),event_id text NOT NULL,
 body_hash text NOT NULL CHECK(body_hash ~ '^[a-f0-9]{64}$'),encrypted_body text,api_version text NOT NULL,event_type text NOT NULL,
 status text NOT NULL DEFAULT 'received' CHECK(status IN ('received','processed','failed','quarantined')),conflict_hash text,
 received_at timestamptz NOT NULL DEFAULT now(),processed_at timestamptz,error_code text,
 UNIQUE(environment,event_id),UNIQUE(environment,id)
);
CREATE TABLE webdock_auth.billing_job (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),environment text NOT NULL CHECK(environment IN ('sandbox','live')),customer_id varchar,
 kind text NOT NULL CHECK(kind IN ('webhook','reconcile','catalog','checkout','publish','subscription-change','endpoint')),
 dedupe_key text NOT NULL,event_id text,generation bigint NOT NULL DEFAULT 0,
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','succeeded','failed','needs_review')),
 attempts integer NOT NULL DEFAULT 0 CHECK(attempts>=0),available_at timestamptz NOT NULL DEFAULT now(),claim_token uuid,lease_until timestamptz,error_code text,
 created_at timestamptz NOT NULL DEFAULT now(),finished_at timestamptz,
 FOREIGN KEY(environment,customer_id) REFERENCES webdock_auth.billing_customer(environment,customer_id) ON DELETE RESTRICT,
 FOREIGN KEY(environment,event_id) REFERENCES webdock_auth.billing_webhook_event(environment,id) ON DELETE RESTRICT,
 UNIQUE(environment,dedupe_key)
);
CREATE INDEX billing_job_due ON webdock_auth.billing_job(environment,available_at) WHERE status IN ('pending','running');
CREATE FUNCTION webdock_auth.billing_immutable() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_TABLE_NAME='billing_product_version' THEN
  IF OLD.frozen THEN RAISE EXCEPTION 'Published billing conditions are immutable' USING ERRCODE='55000';END IF;
 ELSIF TG_OP='DELETE' OR (to_jsonb(NEW)-ARRAY['state','service_status','ended_at','projected_hash','projected_at']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['state','service_status','ended_at','projected_hash','projected_at']) THEN
  RAISE EXCEPTION 'Accepted billing conditions are immutable' USING ERRCODE='55000';
 END IF;
 IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
END $$;
CREATE TRIGGER billing_version_immutable BEFORE UPDATE OR DELETE ON webdock_auth.billing_product_version FOR EACH ROW EXECUTE FUNCTION webdock_auth.billing_immutable();
CREATE TRIGGER billing_contract_immutable BEFORE UPDATE OR DELETE ON webdock_auth.billing_contract FOR EACH ROW EXECUTE FUNCTION webdock_auth.billing_immutable();
ALTER FUNCTION webdock_auth.billing_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.billing_immutable() FROM PUBLIC;
-- Closed by default. Explicit signed policies installed below; never a broad worker grant.
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['billing_connection','billing_product','billing_product_version','billing_product_mapping','billing_customer','billing_contract','billing_checkout','billing_subscription','billing_order','billing_webhook_event','billing_job'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',t);
  EXECUTE format('CREATE POLICY billing_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
 END LOOP;
END $$;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_connection TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_connection FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_connection FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_connection FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_connection TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_connection FOR SELECT TO webdock_worker_runtime USING(false);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_connection FOR INSERT TO webdock_worker_runtime WITH CHECK(false);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_connection FOR UPDATE TO webdock_worker_runtime USING(false) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_product FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND visibility='public' AND operationally_ready AND NOT archived));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_product FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_product FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND visibility='public' AND operationally_ready AND NOT archived)) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_product FOR SELECT TO webdock_worker_runtime USING(false);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_product FOR INSERT TO webdock_worker_runtime WITH CHECK(false);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_product FOR UPDATE TO webdock_worker_runtime USING(false) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product_version TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_product_version FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND EXISTS(SELECT 1 FROM webdock_auth.billing_product p WHERE p.id=product_id AND p.visibility='public' AND p.operationally_ready AND NOT p.archived)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_product_version FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_product_version FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND EXISTS(SELECT 1 FROM webdock_auth.billing_product p WHERE p.id=product_id AND p.visibility='public' AND p.operationally_ready AND NOT p.archived))) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product_version TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_product_version FOR SELECT TO webdock_worker_runtime USING(false);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_product_version FOR INSERT TO webdock_worker_runtime WITH CHECK(false);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_product_version FOR UPDATE TO webdock_worker_runtime USING(false) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product_mapping TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_product_mapping FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND status='ready' AND environment='live' AND EXISTS(SELECT 1 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id WHERE v.id=version_id AND p.visibility='public' AND p.operationally_ready AND NOT p.archived)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_product_mapping FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_product_mapping FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND status='ready' AND environment='live' AND EXISTS(SELECT 1 FROM webdock_auth.billing_product_version v JOIN webdock_auth.billing_product p ON p.id=v.product_id WHERE v.id=version_id AND p.visibility='public' AND p.operationally_ready AND NOT p.archived))) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_product_mapping TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_product_mapping FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR webdock_rls.worker(ARRAY['polar:'||environment||':customer']));
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_product_mapping FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':catalog']));
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_product_mapping FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR webdock_rls.worker(ARRAY['polar:'||environment||':customer'])) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':catalog']));
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_customer TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_customer FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_customer FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_customer FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true))) WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_customer TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_customer FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_customer FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_customer FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_contract TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_contract FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_contract FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_contract FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true))) WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_contract TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_contract FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_contract FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_contract FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_checkout TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_checkout FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_checkout FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_checkout FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true))) WITH CHECK(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_checkout TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_checkout FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_checkout FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_checkout FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_subscription TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_subscription FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_subscription FOR INSERT TO webdock_auth_runtime WITH CHECK(false);
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_subscription FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true))) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_subscription TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_subscription FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_subscription FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_subscription FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_order TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_order FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true)));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_order FOR INSERT TO webdock_auth_runtime WITH CHECK(false);
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_order FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator() OR ((webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb) AND webdock_rls.customer_allowed(customer_id::text,true))) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_order TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_order FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_order FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_order FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_webhook_event TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_webhook_event FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_webhook_event FOR INSERT TO webdock_auth_runtime WITH CHECK(false);
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_webhook_event FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(false);
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_webhook_event TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_webhook_event FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']));
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_webhook_event FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']));
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_webhook_event FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher'])) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']));
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_job TO webdock_auth_runtime;
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.billing_job FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.billing_job FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.billing_job FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.billing_job TO webdock_worker_runtime;
CREATE POLICY webdock_worker_runtime_read ON webdock_auth.billing_job FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']) OR webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR (webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text));
CREATE POLICY webdock_worker_runtime_insert ON webdock_auth.billing_job FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']) OR webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR (webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text));
CREATE POLICY webdock_worker_runtime_update ON webdock_auth.billing_job FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']) OR webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR (webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text)) WITH CHECK(webdock_rls.worker(ARRAY['polar:'||environment||':inbox']) OR webdock_rls.worker(ARRAY['polar:'||environment||':dispatcher']) OR webdock_rls.worker(ARRAY['polar:'||environment||':catalog']) OR (webdock_rls.worker(ARRAY['polar:'||environment||':customer']) AND webdock_rls.request_scope()->>'customerID'=customer_id::text));
