-- Platform-assigned public HTTP names. Host reservations outlive resource deletion.
CREATE TABLE webdock_auth.public_hosting_domain (
 domain text PRIMARY KEY CHECK (char_length(domain) BETWEEN 3 AND 189 AND domain ~ '^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
 cluster_id varchar UNIQUE REFERENCES webdock_auth.hosting_cluster(id) ON DELETE SET NULL,
 enabled boolean NOT NULL DEFAULT false,
 tenant_daily_limit integer NOT NULL CHECK (tenant_daily_limit BETWEEN 1 AND 1000),
 global_daily_limit integer NOT NULL CHECK (global_daily_limit BETWEEN tenant_daily_limit AND 1000),
 trusted_customers text[] NOT NULL DEFAULT '{}' CHECK (cardinality(trusted_customers)<=100),
 revision bigint NOT NULL DEFAULT 1 CHECK (revision BETWEEN 1 AND 9007199254740991),
 configured_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE webdock_auth.public_hosting_domain OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.public_hosting_domain ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.public_hosting_domain FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.public_hosting_domain FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.public_hosting_domain TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY operator_policy ON webdock_auth.public_hosting_domain TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
GRANT SELECT,INSERT,UPDATE ON webdock_auth.public_hosting_domain TO webdock_auth_runtime;

-- Existing hosting_app_project_identity already binds id/project_id.
CREATE UNIQUE INDEX hosting_project_public_identity ON webdock_auth.hosting_project(project_id,customer_id,cluster_id);
CREATE TABLE webdock_auth.platform_hostname (
 id text PRIMARY KEY CHECK (id ~ '^[1-9][0-9]{0,18}$' AND id::numeric<=9223372036854775807),
 domain text NOT NULL REFERENCES webdock_auth.public_hosting_domain(domain) ON DELETE RESTRICT,
 label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 63 AND label ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
 hostname text NOT NULL UNIQUE CHECK (hostname=label||'.'||domain AND char_length(hostname)<=253),
 customer_id varchar REFERENCES webdock_admin.customers(id) ON DELETE SET NULL,
 project_id varchar,
 app_id varchar UNIQUE,
 cluster_id varchar REFERENCES webdock_auth.hosting_cluster(id) ON DELETE SET NULL,
 allocated_customer_id text NOT NULL,
 state text NOT NULL DEFAULT 'allocated' CHECK (state IN ('allocated','ready','retired')),
 revision bigint NOT NULL DEFAULT 1 CHECK (revision BETWEEN 1 AND 9007199254740991),
 created_by text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 retired_at timestamptz,
 CHECK (customer_id IS NULL OR allocated_customer_id=customer_id),
 FOREIGN KEY(app_id,project_id) REFERENCES webdock_auth.hosting_app(id,project_id) ON DELETE SET NULL,
 FOREIGN KEY(project_id,customer_id,cluster_id) REFERENCES webdock_auth.hosting_project(project_id,customer_id,cluster_id) ON DELETE SET NULL
);
CREATE INDEX platform_hostname_allocation_budget ON webdock_auth.platform_hostname(domain,created_at,allocated_customer_id);
ALTER TABLE webdock_auth.platform_hostname OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.platform_hostname ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.platform_hostname FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.platform_hostname FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.platform_hostname TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY project_read ON webdock_auth.platform_hostname FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator() OR webdock_rls.project_permission(project_id,'org:projects:read'));
CREATE POLICY agent_read ON webdock_auth.platform_hostname FOR SELECT TO webdock_worker_runtime USING(webdock_rls.agent_project(project_id));
CREATE POLICY agent_publish ON webdock_auth.platform_hostname FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.agent_project(project_id)) WITH CHECK(webdock_rls.agent_project(project_id));
GRANT SELECT ON webdock_auth.platform_hostname TO webdock_auth_runtime;
GRANT SELECT,UPDATE(state) ON webdock_auth.platform_hostname TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.fence_platform_hostname() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.id<>OLD.id OR NEW.domain<>OLD.domain OR NEW.label<>OLD.label OR NEW.hostname<>OLD.hostname
    OR NEW.allocated_customer_id<>OLD.allocated_customer_id OR NEW.created_by<>OLD.created_by OR NEW.created_at<>OLD.created_at
    OR (NEW.customer_id IS NOT NULL AND NEW.customer_id IS DISTINCT FROM OLD.customer_id)
    OR (NEW.project_id IS NOT NULL AND NEW.project_id IS DISTINCT FROM OLD.project_id)
    OR (NEW.app_id IS NOT NULL AND NEW.app_id IS DISTINCT FROM OLD.app_id)
    OR (NEW.cluster_id IS NOT NULL AND NEW.cluster_id IS DISTINCT FROM OLD.cluster_id)
    OR (OLD.state='retired' AND NEW.state<>'retired') THEN
  RAISE EXCEPTION 'Public hostname ownership is immutable' USING ERRCODE='42501';
 END IF;
 IF NEW.customer_id IS NULL OR NEW.project_id IS NULL OR NEW.app_id IS NULL OR NEW.cluster_id IS NULL THEN NEW.state:='retired'; END IF;
 IF NEW.state='retired' THEN
  NEW.retired_at:=coalesce(OLD.retired_at,clock_timestamp());
  NEW.revision:=OLD.revision+CASE WHEN OLD.state<>'retired' THEN 1 ELSE 0 END;
 ELSE
  NEW.retired_at:=OLD.retired_at;NEW.revision:=OLD.revision;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.fence_platform_hostname() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.fence_platform_hostname() FROM PUBLIC;
CREATE TRIGGER platform_hostname_fence BEFORE UPDATE ON webdock_auth.platform_hostname FOR EACH ROW EXECUTE FUNCTION webdock_rls.fence_platform_hostname();

CREATE FUNCTION webdock_rls.platform_host_reserved(host text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.public_hosting_domain d WHERE host=d.domain OR right(host,char_length(d.domain)+1)='.'||d.domain OR right(d.domain,char_length(host)+1)='.'||host)
$$;
ALTER FUNCTION webdock_rls.platform_host_reserved(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.platform_host_reserved(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.platform_host_reserved(text) TO webdock_auth_runtime,webdock_worker_runtime;

CREATE FUNCTION webdock_rls.public_site_policy(project text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE result jsonb;
BEGIN
 IF webdock_rls.project_permission(project,'org:projects:read') IS DISTINCT FROM true THEN RETURN NULL; END IF;
 SELECT jsonb_build_object('domain',d.domain,'enabled',d.enabled) INTO result
 FROM webdock_auth.hosting_project p JOIN webdock_auth.public_hosting_domain d ON d.cluster_id=p.cluster_id
 JOIN webdock_auth.hosting_cluster c ON c.id=p.cluster_id
 WHERE p.project_id=project AND p.provider='k3s' AND c.runtime_kind='kubernetes' AND c.ownership='platform' AND c.verified
   AND (webdock_rls.operator() OR p.customer_id=ANY(d.trusted_customers));
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.public_site_policy(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.public_site_policy(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.public_site_policy(text) TO webdock_auth_runtime;

CREATE FUNCTION webdock_rls.allocate_platform_hostname(allocation text,app text,requested_label text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v_project text;v_customer text;v_cluster text;v_name text;v_provider text;v_runtime text;v_verified boolean;v_ownership text;
 v_policy webdock_auth.public_hosting_domain%ROWTYPE;v_row webdock_auth.platform_hostname%ROWTYPE;
 v_label text;v_fallback text;v_explicit boolean:=requested_label IS NOT NULL;v_start timestamptz;
 reserved text[]:=ARRAY['admin','api','auth','console','studio','docs','registry','grafana','prometheus','status','www','mail','webmail','support','spitzli','spitzli-cloud','webdock'];
BEGIN
 IF allocation IS NULL OR allocation !~ '^[1-9][0-9]{0,18}$' OR allocation::numeric>9223372036854775807 THEN RAISE EXCEPTION 'Invalid allocation ID' USING ERRCODE='22023'; END IF;
 SELECT a.project_id,h.customer_id,h.cluster_id,p.name,h.provider,c.runtime_kind,c.verified,c.ownership
 INTO v_project,v_customer,v_cluster,v_name,v_provider,v_runtime,v_verified,v_ownership
 FROM webdock_auth.hosting_app a JOIN webdock_auth.hosting_project h ON h.project_id=a.project_id JOIN webdock_auth.hosting_cluster c ON c.id=h.cluster_id JOIN webdock_admin.projects p ON p.id=a.project_id
 WHERE a.id=app AND a.status<>'deleted' FOR UPDATE OF a;
 IF v_project IS NULL OR webdock_rls.project_permission(v_project,'org:projects:manage') IS DISTINCT FROM true THEN RAISE EXCEPTION 'Public address access denied' USING ERRCODE='42501'; END IF;
 IF v_provider<>'k3s' OR v_runtime<>'kubernetes' OR v_ownership<>'platform' OR NOT v_verified THEN RAISE EXCEPTION 'Public routing is unavailable on this cluster' USING ERRCODE='42501'; END IF;
 SELECT * INTO v_row FROM webdock_auth.platform_hostname WHERE app_id=app;
 IF FOUND THEN
  IF v_row.state='retired' OR (v_explicit AND requested_label<>v_row.label) THEN RAISE EXCEPTION 'Public address is immutable' USING ERRCODE='23505'; END IF;
  RETURN jsonb_build_object('id',v_row.id,'hostname',v_row.hostname,'revision',v_row.revision,'state',v_row.state);
 END IF;
 SELECT * INTO v_policy FROM webdock_auth.public_hosting_domain WHERE cluster_id=v_cluster FOR UPDATE;
 IF NOT FOUND OR NOT v_policy.enabled OR NOT (webdock_rls.operator() OR v_customer=ANY(v_policy.trusted_customers)) THEN RAISE EXCEPTION 'Public address admission is disabled' USING ERRCODE='42501'; END IF;
 IF v_explicit THEN
  v_label:=requested_label;
  IF char_length(v_label) NOT BETWEEN 1 AND 63 OR v_label !~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$' OR v_label=ANY(reserved) OR v_label ~ '^p-[0-9]+$' THEN RAISE EXCEPTION 'Public address name is invalid or reserved' USING ERRCODE='22023'; END IF;
 ELSE
  v_label:=btrim(left(regexp_replace(lower(v_name),'[^a-z0-9]+','-','g'),63),'-');
  IF v_label='' OR v_label=ANY(reserved) OR v_label ~ '^p-[0-9]+$' THEN v_label:='p-'||app; END IF;
 END IF;
 v_fallback:='p-'||app;
 IF EXISTS(SELECT 1 FROM webdock_auth.platform_hostname WHERE hostname=v_label||'.'||v_policy.domain) THEN
  IF v_explicit THEN RAISE EXCEPTION 'Public address name is unavailable' USING ERRCODE='23505'; END IF;
  v_label:=v_fallback;
 END IF;
 v_start:=date_trunc('day',clock_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC';
 IF (SELECT count(*) FROM webdock_auth.platform_hostname WHERE domain=v_policy.domain AND created_at>=v_start AND allocated_customer_id=v_customer)>=v_policy.tenant_daily_limit
 OR (SELECT count(*) FROM webdock_auth.platform_hostname WHERE domain=v_policy.domain AND created_at>=v_start)>=v_policy.global_daily_limit THEN RAISE EXCEPTION 'Public address allocation is temporarily limited' USING ERRCODE='54000'; END IF;
 INSERT INTO webdock_auth.platform_hostname(id,domain,label,hostname,customer_id,allocated_customer_id,project_id,app_id,cluster_id,created_by)
 VALUES(allocation,v_policy.domain,v_label,v_label||'.'||v_policy.domain,v_customer,v_customer,v_project,app,v_cluster,webdock_rls.subject()) RETURNING * INTO v_row;
 RETURN jsonb_build_object('id',v_row.id,'hostname',v_row.hostname,'revision',v_row.revision,'state',v_row.state);
END $$;
ALTER FUNCTION webdock_rls.allocate_platform_hostname(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.allocate_platform_hostname(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.allocate_platform_hostname(text,text,text) TO webdock_auth_runtime;

-- Keep every custom/plugin path outside platform assignments, including already-verified parents.
-- The same lock closes the race between policy creation, domain verification and workload pinning.
CREATE FUNCTION webdock_rls.fence_public_domain_overlap() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(hashtextextended('webdock-public-domain-authority',0));
 IF TG_TABLE_NAME='public_hosting_domain' THEN
  IF EXISTS(SELECT 1 FROM webdock_auth.customer_domain d WHERE d.verified_at IS NOT NULL AND
       (d.domain=NEW.domain OR right(d.domain,char_length(NEW.domain)+1)='.'||NEW.domain OR right(NEW.domain,char_length(d.domain)+1)='.'||d.domain))
     OR EXISTS(SELECT 1 FROM webdock_auth.plugin_workload w,unnest(w.hosts) h WHERE NOT w.removing AND
       (h=NEW.domain OR right(h,char_length(NEW.domain)+1)='.'||NEW.domain OR right(NEW.domain,char_length(h)+1)='.'||h)) THEN
   RAISE EXCEPTION 'Public hosting domain overlaps an existing verified domain or workload' USING ERRCODE='42501';
  END IF;
 ELSIF TG_TABLE_NAME='customer_domain' THEN
  IF webdock_rls.platform_host_reserved(NEW.domain) THEN
   RAISE EXCEPTION 'Platform hosting names cannot be claimed as custom domains' USING ERRCODE='42501';
  END IF;
 ELSIF TG_TABLE_NAME='plugin_workload' THEN
  IF EXISTS(SELECT 1 FROM unnest(NEW.hosts) h WHERE webdock_rls.platform_host_reserved(h)) THEN
   RAISE EXCEPTION 'Platform hosting names require an exact native allocation' USING ERRCODE='42501';
  END IF;
 ELSE RAISE EXCEPTION 'Invalid public domain authority target' USING ERRCODE='42501';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.fence_public_domain_overlap() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.fence_public_domain_overlap() FROM PUBLIC;
CREATE TRIGGER fence_public_policy BEFORE INSERT OR UPDATE OF domain ON webdock_auth.public_hosting_domain FOR EACH ROW EXECUTE FUNCTION webdock_rls.fence_public_domain_overlap();
CREATE TRIGGER fence_public_custom_domain BEFORE INSERT OR UPDATE OF domain,verified_at ON webdock_auth.customer_domain FOR EACH ROW EXECUTE FUNCTION webdock_rls.fence_public_domain_overlap();
CREATE TRIGGER fence_public_plugin_hosts BEFORE INSERT OR UPDATE OF hosts ON webdock_auth.plugin_workload FOR EACH ROW EXECUTE FUNCTION webdock_rls.fence_public_domain_overlap();

-- Draft deletion has no agent invocation; retire its reservation atomically with the app state.
CREATE FUNCTION webdock_rls.retire_deleted_app_hostname() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 UPDATE webdock_auth.platform_hostname SET state='retired' WHERE app_id=NEW.id AND state<>'retired';
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.retire_deleted_app_hostname() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.retire_deleted_app_hostname() FROM PUBLIC;
CREATE TRIGGER retire_deleted_app_hostname AFTER UPDATE OF status ON webdock_auth.hosting_app FOR EACH ROW WHEN(NEW.status='deleted' AND OLD.status IS DISTINCT FROM NEW.status) EXECUTE FUNCTION webdock_rls.retire_deleted_app_hostname();
