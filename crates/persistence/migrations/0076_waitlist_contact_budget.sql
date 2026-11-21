-- Charge the intake budget for new contacts, not retries or per-email throttling.
-- Preserve all existing contacts, review decisions and active rate-limit windows.
CREATE OR REPLACE FUNCTION webdock_auth.submit_waitlist(entry_id text,contact text,contact_name text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE key_email text; count_email integer; count_global integer; stamp bigint; inserted integer;
BEGIN
 IF entry_id !~ '^[1-9][0-9]{0,18}$' OR contact IS NULL OR contact<>lower(btrim(contact))
    OR char_length(contact) NOT BETWEEN 3 AND 254 OR contact !~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$'
    OR split_part(contact,'@',1) LIKE '.%' OR split_part(contact,'@',1) LIKE '%.' OR position('..' IN contact)>0
    OR char_length(contact_name)>100 OR contact_name ~ '[[:cntrl:]]' THEN
  RAISE EXCEPTION 'Invalid waitlist input' USING ERRCODE='22023';
 END IF;
 stamp:=floor(extract(epoch FROM statement_timestamp())*1000)::bigint;
 -- Lock the shared window before contact lookup. A full window rejects every email
 -- identically and creates no new per-email rows; concurrent inserts cannot exceed it.
 INSERT INTO webdock_auth."rateLimit"(id,key,count,"lastRequest") VALUES('waitlist:global','waitlist:global',0,stamp)
 ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-86400000 THEN 0 ELSE webdock_auth."rateLimit".count END,
 "lastRequest"=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-86400000 THEN stamp ELSE webdock_auth."rateLimit"."lastRequest" END RETURNING count INTO count_global;
 IF count_global>=1000 THEN RETURN false; END IF;
 key_email:='waitlist:email:'||encode(pg_catalog.sha256(pg_catalog.convert_to(contact,'UTF8')),'hex');
 INSERT INTO webdock_auth."rateLimit"(id,key,count,"lastRequest") VALUES(key_email,key_email,1,stamp)
 ON CONFLICT(key) DO UPDATE SET count=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-60000 THEN 1 ELSE least(webdock_auth."rateLimit".count,5)+1 END,
 "lastRequest"=CASE WHEN webdock_auth."rateLimit"."lastRequest"<=stamp-60000 THEN stamp ELSE webdock_auth."rateLimit"."lastRequest" END RETURNING count INTO count_email;
 IF count_email>5 THEN RETURN false; END IF;
 INSERT INTO webdock_auth.waitlist_entry(id,email,name) VALUES(entry_id,contact,nullif(btrim(contact_name),'')) ON CONFLICT(email) DO NOTHING;
 GET DIAGNOSTICS inserted = ROW_COUNT;
 IF inserted=1 THEN
  UPDATE webdock_auth."rateLimit" SET count=count+1 WHERE key='waitlist:global';
 END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_auth.submit_waitlist(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.submit_waitlist(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.submit_waitlist(text,text,text) TO webdock_auth_bootstrap;
