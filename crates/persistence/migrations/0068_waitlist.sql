-- Public contact admission is independent of an authenticated/product account.
CREATE TABLE webdock_auth.waitlist_entry (
 id text PRIMARY KEY CHECK(id ~ '^[1-9][0-9]{0,18}$'),
 email text NOT NULL UNIQUE CHECK(email=lower(btrim(email)) AND char_length(email) BETWEEN 3 AND 254),
 name text CHECK(char_length(name)<=100),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','approved','rejected')),
 privacy_version text NOT NULL DEFAULT '2026-10-31',
 created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 reviewed_by text,
 reviewed_at timestamptz,
 version integer NOT NULL DEFAULT 1 CHECK(version>0),
 CHECK((status='pending' AND reviewed_by IS NULL AND reviewed_at IS NULL) OR (status<>'pending' AND reviewed_by IS NOT NULL AND reviewed_at IS NOT NULL))
);
ALTER TABLE webdock_auth.waitlist_entry OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.waitlist_entry ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.waitlist_entry FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.waitlist_entry FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.waitlist_entry TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY operator_review ON webdock_auth.waitlist_entry TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
GRANT SELECT ON webdock_auth.waitlist_entry TO webdock_auth_runtime;
GRANT UPDATE(status,reviewed_by,reviewed_at,version) ON webdock_auth.waitlist_entry TO webdock_auth_runtime;
CREATE INDEX waitlist_review_order ON webdock_auth.waitlist_entry(status,id DESC);

-- Return identical success for new/existing/approved/rejected contacts. No account is created.
CREATE FUNCTION webdock_auth.submit_waitlist(entry_id text,contact text,contact_name text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE key_email text; count_email integer; count_global integer; stamp bigint;
BEGIN
 IF entry_id !~ '^[1-9][0-9]{0,18}$' OR contact IS NULL OR contact<>lower(btrim(contact))
    OR char_length(contact) NOT BETWEEN 3 AND 254 OR contact !~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$'
    OR split_part(contact,'@',1) LIKE '.%' OR split_part(contact,'@',1) LIKE '%.' OR position('..' IN contact)>0
    OR char_length(contact_name)>100 OR contact_name ~ '[[:cntrl:]]' THEN
  RAISE EXCEPTION 'Invalid waitlist input' USING ERRCODE='22023';
 END IF;
 stamp:=floor(extract(epoch FROM statement_timestamp())*1000)::bigint;
 -- Initial invite-only intake budget bounds storage growth without touching login rate limits.
 INSERT INTO webdock_auth."rateLimit"(id,key,count,"lastRequest") VALUES('waitlist:global','waitlist:global',1,stamp)
 ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-86400000 THEN 1 ELSE webdock_auth."rateLimit".count+1 END,
 "lastRequest"=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-86400000 THEN stamp ELSE webdock_auth."rateLimit"."lastRequest" END RETURNING count INTO count_global;
 IF count_global>1000 THEN RETURN false; END IF;
 key_email:='waitlist:email:'||encode(pg_catalog.sha256(pg_catalog.convert_to(contact,'UTF8')),'hex');
 INSERT INTO webdock_auth."rateLimit"(id,key,count,"lastRequest") VALUES(key_email,key_email,1,stamp)
 ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-60000 THEN 1 ELSE webdock_auth."rateLimit".count+1 END,
 "lastRequest"=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-60000 THEN stamp ELSE webdock_auth."rateLimit"."lastRequest" END RETURNING count INTO count_email;
 IF count_email>5 THEN RETURN false; END IF;
 INSERT INTO webdock_auth.waitlist_entry(id,email,name) VALUES(entry_id,contact,nullif(btrim(contact_name),'')) ON CONFLICT(email) DO NOTHING;
 RETURN true;
END $$;
ALTER FUNCTION webdock_auth.submit_waitlist(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.submit_waitlist(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.submit_waitlist(text,text,text) TO webdock_auth_bootstrap;
CREATE TRIGGER live_waitlist_revision AFTER INSERT OR UPDATE OR DELETE ON webdock_auth.waitlist_entry FOR EACH STATEMENT EXECUTE FUNCTION webdock_live.invalidate_directory();
