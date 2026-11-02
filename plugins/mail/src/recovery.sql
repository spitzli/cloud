-- Physical migration 5: a safety interlock, never a second send/backup ledger.
CREATE TABLE mail_recovery_control (
 singleton boolean PRIMARY KEY CHECK(singleton),
 epoch uuid NOT NULL,
 state text NOT NULL CHECK(state IN ('active','fenced','quarantined')),
 operation uuid,
 generation bigint NOT NULL DEFAULT 0 CHECK(generation>=0),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0)
);
INSERT INTO mail_recovery_control(singleton,epoch,state) VALUES(true,gen_random_uuid(),'active');
-- Recovery effect custody only, never message/attempt state. Cancellation/connection
-- loss must not look like a completed external effect when an advisory lock vanishes.
CREATE TABLE mail_recovery_effect (
 token uuid PRIMARY KEY,
 epoch uuid NOT NULL
);

CREATE FUNCTION mail_recovery_write_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
 IF NOT pg_try_advisory_xact_lock_shared(704612,1) THEN
  RAISE EXCEPTION 'mail recovery fenced' USING ERRCODE='55000';
 END IF;
 -- A locking read also rejects a pre-fence repeatable-read snapshot.
 PERFORM singleton FROM public.mail_recovery_control WHERE singleton AND state='active' FOR SHARE;
 IF NOT FOUND THEN RAISE EXCEPTION 'mail recovery fenced' USING ERRCODE='55000'; END IF;
 RETURN NULL;
END $$;
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['domains','relays','sends','engines','project_customers','customer_engines','mailboxes','private_payload_key','private_payloads'] LOOP
  EXECUTE format('CREATE TRIGGER mail_recovery_write BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON %I FOR EACH STATEMENT EXECUTE FUNCTION mail_recovery_write_guard()',t);
 END LOOP;
END $$;
