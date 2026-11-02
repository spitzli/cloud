-- Native0031: domains customers prove they own (a TXT record with a Core-made token), and the hosts
-- each plugin unit routes or certifies. Core lets a unit use a host only under the plugin's own route
-- domains or under a domain its project's customer verified. A verified domain covers its subdomains.
CREATE TABLE webdock_auth.customer_domain (
 id text PRIMARY KEY CHECK (id ~ '^[1-9][0-9]{0,18}$'),
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 domain text NOT NULL CHECK (char_length(domain)<=253 AND domain ~ '^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
 token text NOT NULL CHECK (token ~ '^[a-f0-9]{32}$'),
 verified_at timestamptz,
 checked_at timestamptz,
 created_by text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE (customer_id, domain)
);
-- Only one customer can own a domain; others may still claim it until someone verifies it.
CREATE UNIQUE INDEX customer_domain_verified ON webdock_auth.customer_domain(domain) WHERE verified_at IS NOT NULL;
ALTER TABLE webdock_auth.customer_domain OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.customer_domain ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.customer_domain FORCE ROW LEVEL SECURITY;
REVOKE ALL ON TABLE webdock_auth.customer_domain FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.customer_domain TO webdock_platform_owner USING (true) WITH CHECK (true);
CREATE POLICY customer_read ON webdock_auth.customer_domain FOR SELECT TO webdock_auth_runtime USING (webdock_rls.customer_allowed(customer_id));
CREATE POLICY customer_insert ON webdock_auth.customer_domain FOR INSERT TO webdock_auth_runtime WITH CHECK (webdock_rls.customer_allowed(customer_id,true));
CREATE POLICY customer_update ON webdock_auth.customer_domain FOR UPDATE TO webdock_auth_runtime USING (webdock_rls.customer_allowed(customer_id,true)) WITH CHECK (webdock_rls.customer_allowed(customer_id,true));
CREATE POLICY customer_delete ON webdock_auth.customer_domain FOR DELETE TO webdock_auth_runtime USING (webdock_rls.customer_allowed(customer_id,true));
GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE webdock_auth.customer_domain TO webdock_auth_runtime;

ALTER TABLE webdock_auth.plugin_workload ADD COLUMN hosts text[] NOT NULL DEFAULT '{}' CHECK (cardinality(hosts)<=64);
CREATE INDEX plugin_workload_hosts ON webdock_auth.plugin_workload USING gin (hosts);

-- The first of `hosts` no verified domain of the project's customer covers, or NULL.
CREATE FUNCTION webdock_rls.project_unverified_host(project text, hosts text[]) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN (SELECT h FROM unnest(hosts) WITH ORDINALITY AS t(h,n)
  WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.customer_domain d JOIN webdock_admin.projects p ON p.customer_id=d.customer_id
   WHERE p.id=project AND d.verified_at IS NOT NULL AND (t.h=d.domain OR right(t.h,char_length(d.domain)+1)='.'||d.domain))
  ORDER BY n LIMIT 1);
END $$;
ALTER FUNCTION webdock_rls.project_unverified_host(text,text[]) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_unverified_host(text,text[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_unverified_host(text,text[]) TO webdock_worker_runtime;

-- Which of the customer's units use a domain or its subdomains, by project; for the customer's eyes.
CREATE FUNCTION webdock_rls.domain_users(customer text, target text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.customer_allowed(customer) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('plugin',w.plugin_id,'unit',w.unit,'project',p.name,
   'resource',(SELECT r.name FROM webdock_auth.plugin_resource r WHERE r.plugin_id=w.plugin_id AND r.resource_id=w.resource_id AND r.project_id=w.project_id)) ORDER BY p.name,w.unit)
  FROM webdock_auth.plugin_workload w JOIN webdock_admin.projects p ON p.id=w.project_id
  WHERE p.customer_id=customer AND NOT w.removing
   AND EXISTS(SELECT 1 FROM unnest(w.hosts) h WHERE h=target OR right(h,char_length(target)+1)='.'||target)),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.domain_users(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.domain_users(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.domain_users(text,text) TO webdock_auth_runtime;
