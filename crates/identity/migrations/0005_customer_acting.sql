-- Boolean mode lookup only: authentication never receives direct tenant access.
-- to_jsonb keeps the helper usable by minimal identity-only fixtures with old preview columns.
CREATE OR REPLACE FUNCTION webdock_rls.identity_is_acting(subject text,session_id text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview p
 JOIN webdock_auth."user" u ON u.id=p.actor_id
 WHERE p.actor_id=$1 AND p.session_id=$2 AND to_jsonb(p)->>'mode'='owner'
 AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified" AND NOT u."mustChangePassword" AND NOT coalesce(u.banned,false)
 AND (to_jsonb(p)->>'expired_at') IS NULL AND (to_jsonb(p)->>'expires_at')::timestamptz>statement_timestamp())
$$;
REVOKE ALL ON FUNCTION webdock_rls.identity_is_acting(text,text) FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') THEN
  ALTER FUNCTION webdock_rls.identity_is_acting(text,text) OWNER TO webdock_platform_owner;
 END IF;
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT EXECUTE ON FUNCTION webdock_rls.identity_is_acting(text,text) TO webdock_auth_bootstrap;
 END IF;
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.identity_customer_scope(subject text,session_id text) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT md5(concat_ws(':',to_jsonb(p)->>'scope_id',p.customer_id,p.organization_id,to_jsonb(p)->>'mode'))
 FROM webdock_auth.studio_tenant_preview p WHERE p.actor_id=$1 AND p.session_id=$2
$$;
REVOKE ALL ON FUNCTION webdock_rls.identity_customer_scope(text,text) FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') THEN
  ALTER FUNCTION webdock_rls.identity_customer_scope(text,text) OWNER TO webdock_platform_owner;
 END IF;
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT EXECUTE ON FUNCTION webdock_rls.identity_customer_scope(text,text) TO webdock_auth_bootstrap;
 END IF;
END $$;
