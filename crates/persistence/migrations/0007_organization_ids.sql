-- Native writers allocate before entering the trigger. Legacy writers remain compatible
-- during the bounded overlap; disabling legacy defaults is a separate final cutover.
CREATE OR REPLACE FUNCTION webdock_auth.sync_customer_tenant() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE tenant_id text; allocation jsonb; raw_hint text;
BEGIN
 SELECT organization_id INTO tenant_id FROM webdock_auth.tenant_customer WHERE customer_id=NEW.id;
 IF tenant_id IS NULL THEN
   raw_hint := nullif(current_setting('webdock.organization_allocation',true),'');
   IF raw_hint IS NULL THEN
     tenant_id := webdock_auth.next_snowflake();
   ELSE
     allocation := raw_hint::jsonb;
     IF jsonb_typeof(allocation)<>'object' OR allocation->>'customer_id' IS DISTINCT FROM NEW.id
        OR jsonb_typeof(allocation->'organization_id') IS DISTINCT FROM 'string'
        OR allocation->>'organization_id' !~ '^[1-9][0-9]{0,18}$' THEN
       RAISE EXCEPTION 'Invalid transaction-bound organization allocation' USING ERRCODE='23514';
     END IF;
     tenant_id := allocation->>'organization_id';
     IF tenant_id::numeric > 9223372036854775807 THEN
       RAISE EXCEPTION 'Organization allocation exceeds ID range' USING ERRCODE='23514';
     END IF;
     -- Consume the hint even when the caller later inserts another customer in this transaction.
     PERFORM set_config('webdock.organization_allocation','',true);
   END IF;
   INSERT INTO webdock_auth.organization(id,name,slug,"createdAt")
   VALUES(tenant_id,NEW.name,'customer-' || NEW.id,now());
   INSERT INTO webdock_auth.tenant_customer(customer_id,organization_id) VALUES(NEW.id,tenant_id);
 ELSE
   UPDATE webdock_auth.organization SET name=NEW.name WHERE id=tenant_id;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.sync_customer_tenant() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.sync_customer_tenant() FROM PUBLIC;
