import { pluginTemplateSchemaSQL } from "../modules/hosting/plugin-template-schema";
import { pluginProvisioningRlsSQL, pluginProvisioningPolicy } from "./plugin-provisioning-rls";
import {pluginScopeSQL,pluginInvocationFenceSQL,mailPluginPolicySQL} from "../modules/plugins/scope-rls";
import {pluginTables,pluginPolicy} from "../modules/plugins/rls";
import { modulePreflightTables } from "./module-preflight-tables";
import { gitInvalidationSQL } from "./database-git-rls";
import {
  setupClerkRevalidationSQL,
  setupLegacyRevalidationSQL,
} from "./setup-clerk-revalidation";
import {
  clerkActorSQL,
  clerkLegacyActorSQL,
  clerkCustomerSQL,
  clerkSetupSQL,
  clerkBillingSQL,
  clerkBillingReadSQL,
  clerkMembershipSQL,
  clerkPermissionSQL,
  clerkProjectionSQL,
} from "./clerk-rls";
import { setupHelperSQL, setupPolicyFor } from "./database-setup-rls";
import { billingTables, billingPolicy } from "./retained-billing-rls";
import { createHash } from "node:crypto";
import { tenantProfileColumns } from "../modules/access/tenant-schema";
/** Offline-only, catalog-driven RLS installation. Never invoke from a request. */
export type RlsTable = {
  schema: string;
  name: string;
  columns: readonly string[];
};
export type RlsPlan = {
  sql: string;
  unknownTables: string[];
  classifiedTables: string[];
};
const ident = (v: string) => '"' + v.replaceAll('"', '""') + '"';
const literal = (v: string) => "'" + v.replaceAll("'", "''") + "'";
const runtimeRoles = [
  "webdock_auth_bootstrap",
  "webdock_auth_runtime",
  "webdock_worker_runtime",
  "webdock_admin_runtime",
  "webdock_registry_runtime",
  "webdock_runtime",
  "webdock_browser_website",
];
const identity = new Set(
  "clerk_identity_mapping clerk_session_mapping user account session verification twoFactor passkey jwks rateLimit organization member invitation oauthAccessToken oauthClient oauthClientAssertion oauthClientResource oauthConsent oauthRefreshToken oauthResource identity_mfa_challenge identity_totp_replay identity_mfa_enrollment identity_recovery_token identity_delivery_job identity_passkey_ceremony identity_oauth_request identity_browser_flow identity_browser_session".split(
    " ",
  ),
);
const authCustomer = new Set(
  "hosting_setup hosting_service_binding tenant_customer hosting_project hosting_limit hosting_operation hosting_reservation hosting_byok_policy hosting_vercel hosting_vercel_flow hosting_turbosmtp hosting_database hosting_database_offer hosting_database_order git_connection git_repository git_flow git_source git_build git_artifact git_release git_vercel_target plan_offer tenant_subscription subscription_history tenant_storage mail_sender_domain mail_tenant_account mail_tracking_domain mail_tracking_operation mail_tracking_settings mail_connection mail_connection_tracking_settings mail_connection_secret mail_provider_operation mail_connection_domain mail_api_key mail_message mail_message_content mail_recipient mail_event mail_suppression mail_webhook mail_webhook_delivery mail_webhook_attempt mail_batch mail_batch_item mail_idempotency mail_usage_period mail_usage_reservation mail_rate_window mail_request_log mail_import_map".split(
    " ",
  ),
);
const mailCustomer = new Set(
  "service operation instance cluster_reservation domain mailbox identity mailbox_grant configuration configuration_operation access_event webmail_launch webmail_session webmail_write api_key send_transport send_job month_usage budget_reservation provider_binding provider_lease".split(
    " ",
  ),
);
const authGlobal = new Set(
  "hosting_cluster hosting_agent hosting_enrollment hosting_external_state hosting_database_plan hosting_database_pool hosting_database_backend hosting_database_port hosting_rate hosting_audit access_event git_worker git_receipt platform_settings platform_mail_credential plan_template mail_callback_source mail_callback_source_connection mail_callback_inbox mail_job_lease mail_job_run mail_product_migration snowflake_state".split(
    " ",
  ),
);
const adminTables = new Set(
  "audit_events cms_instances customers payload_kv payload_locked_documents payload_locked_documents_rels payload_migrations payload_preferences payload_preferences_rels project_deletion projects snowflake_state users users_sessions vercel_connection vercel_project_link".split(
    " ",
  ),
);
const websiteTables = new Set(
  "_landing_page_v _landing_page_v_version_faqs _landing_page_v_version_use_cases landing_page landing_page_faqs landing_page_use_cases payload_kv payload_locked_documents payload_locked_documents_rels payload_migrations payload_preferences payload_preferences_rels users users_sessions cms_bridge_transfers cms_bridge_transfer_chunks".split(
    " ",
  ),
);
const childRelations: Record<
  string,
  { parent: string; foreign: string; key: string }
> = {
  hosting_app: {
    parent: "hosting_project",
    foreign: "project_id",
    key: "project_id",
  },
  hosting_database_job: {
    parent: "hosting_operation",
    foreign: "operation_id",
    key: "id",
  },
  hosting_database_migration: {
    parent: "hosting_database",
    foreign: "database_id",
    key: "id",
  },
  hosting_database_preview: {
    parent: "hosting_database",
    foreign: "database_id",
    key: "id",
  },
  git_check_outbox: { parent: "git_build", foreign: "build_id", key: "id" },
  database_grant: {
    parent: "database_binding",
    foreign: "binding_id",
    key: "id",
  },
  database_runtime_owner: {
    parent: "database_binding",
    foreign: "binding_id",
    key: "id",
  },
  database_audit: {
    parent: "database_binding",
    foreign: "binding_id",
    key: "id",
  },
  database_launch: { parent: "database_grant", foreign: "grant_id", key: "id" },
  database_session: {
    parent: "database_grant",
    foreign: "grant_id",
    key: "id",
  },
};
const metadata: Record<string, string[]> = {
  "webdock_admin.users": [
    "id",
    "name",
    "email",
    "role",
    "auth_subject",
    "created_at",
    "updated_at",
  ],
  "webdock_auth.user": [
    "id",
    "email",
    "role",
    "banned",
    "emailVerified",
    "mustChangePassword",
    "twoFactorEnabled",
  ],
  "webdock_auth.session": ["id", "userId", "expiresAt"],
  "webdock_auth.member": ["userId", "organizationId", "role"],
  "webdock_auth.tenant_customer": ["customer_id", "organization_id"],
  "webdock_auth.studio_tenant_preview": [
    "session_id",
    "actor_id",
    "customer_id",
    "organization_id",
    "expires_at",
    "expired_at",
  ],
  "webdock_admin.customers": ["id", "status"],
  "webdock_admin.projects": ["id", "customer_id", "status"],
  "webdock_auth.hosting_project": ["project_id", "customer_id", "cluster_id"],
  "webdock_auth.hosting_database_backend": ["id", "cluster_id", "kind"],
  "webdock_auth.hosting_database": [
    "id",
    "customer_id",
    "backend_id",
    "pending_backend_id",
  ],
  "webdock_auth.hosting_database_migration": [
    "database_id",
    "source_backend_id",
    "target_backend_id",
  ],
  "webdock_auth.hosting_database_order": ["customer_id", "plan_revision_id"],
  "webdock_mail.cluster_reservation": ["customer_id", "cluster_id"],
  "webdock_auth.mail_connection": ["id", "customer_id", "deleted_at"],
  "webdock_auth.mail_api_key": [
    "id",
    "customer_id",
    "connection_id",
    "permission",
    "revoked_at",
  ],
};
Object.assign(metadata, {
  "webdock_auth.clerk_identity_mapping": ["local_subject"],
  "webdock_auth.clerk_identity": [
    "issuer",
    "clerk_user_id",
    "user_id",
    "enabled",
  ],
  "webdock_auth.clerk_organization": [
    "issuer",
    "clerk_organization_id",
    "organization_id",
    "enabled",
  ],
  "webdock_auth.console_tenant_preview": [
    "issuer",
    "session_id",
    "actor_id",
    "customer_id",
    "organization_id",
    "expires_at",
    "expired_at",
  ],
});
const bootstrapMetadata: Record<string, string[]> = {
  "webdock_admin.customers": ["id", "status"],
  "webdock_auth.tenant_customer": ["customer_id", "organization_id"],
  "webdock_auth.app_binding": ["id", "enabled", "organization_id", "client_id"],
  "webdock_auth.project_grant": [
    "id",
    "user_id",
    "binding_id",
    "organization_id",
    "role",
    "enabled",
  ],
  "webdock_auth.platform_settings": ["id", "settings"],
  "webdock_auth.hosting_agent": [
    "cluster_id",
    "generation",
    "credential_hash",
    "revoked",
  ],
  "webdock_auth.hosting_enrollment": [
    "cluster_id",
    "token_hash",
    "consumed_at",
    "expires_at",
  ],
  "webdock_auth.git_worker": ["credential_hash", "enabled"],
  "webdock_auth.mail_api_key": [
    "id",
    "customer_id",
    "connection_id",
    "permission",
    "key_hash",
    "revoked_at",
  ],
  "webdock_auth.mail_callback_source": [
    "id",
    "secret_hash",
    "enabled",
    "verified_at",
  ],
};
const helperSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.native_organization_authority(org text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE allowed boolean;
BEGIN
 IF to_regprocedure('webdock_rls.local_organization_authority(text)') IS NOT NULL THEN
  EXECUTE 'SELECT webdock_rls.local_organization_authority($1)' INTO allowed USING org;
  RETURN coalesce(allowed,false);
 END IF;
 IF to_regclass('webdock_auth.clerk_organization') IS NULL THEN RETURN true; END IF;
 EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1)' INTO allowed USING org;
 RETURN allowed;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.native_identity_authority(subject text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE allowed boolean;
BEGIN
 IF to_regprocedure('webdock_rls.local_identity_authority(text)') IS NOT NULL THEN
  EXECUTE 'SELECT webdock_rls.local_identity_authority($1)' INTO allowed USING subject;
  RETURN coalesce(allowed,false);
 END IF;
 IF to_regclass('webdock_auth.clerk_identity') IS NOT NULL THEN
  EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity WHERE user_id=$1)' INTO allowed USING subject;
  IF NOT allowed THEN RETURN false; END IF;
 END IF;
 IF to_regclass('webdock_auth.clerk_identity_mapping') IS NOT NULL THEN
  EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1)' INTO allowed USING subject;
  IF NOT allowed THEN RETURN false; END IF;
 END IF;
 RETURN true;
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.request_scope() RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE raw text; signature text; envelope jsonb; secret_value text; expiry numeric;
BEGIN
 raw:=current_setting('webdock.rls_context',true); signature:=current_setting('webdock.rls_signature',true);
 IF raw IS NULL OR length(raw)>8192 OR signature IS NULL OR signature !~ '^[a-f0-9]{64}$' THEN RETURN NULL; END IF;
 SELECT secret INTO secret_value FROM webdock_rls.context_key LIMIT 1;
 IF secret_value IS NULL OR length(secret_value)<32 OR encode(webdock_rls.hmac(raw,secret_value,'sha256'),'hex')<>signature THEN RETURN NULL; END IF;
 envelope:=raw::jsonb; expiry:=(envelope->>'exp')::numeric;
 IF envelope->>'v' IS DISTINCT FROM '1' OR envelope->>'aud' IS DISTINCT FROM session_user::text OR session_user NOT IN ('webdock_auth_runtime','webdock_worker_runtime','webdock_registry_runtime') OR expiry IS NULL OR expiry<=extract(epoch FROM statement_timestamp()) OR expiry>extract(epoch FROM statement_timestamp())+120 OR jsonb_typeof(envelope->'scope') IS DISTINCT FROM 'object' THEN RETURN NULL; END IF;
 RETURN envelope->'scope';
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range THEN RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.actor() RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE s jsonb; actor_row record; preview_row record;
BEGIN
 s:=webdock_rls.request_scope();
 IF session_user NOT IN ('webdock_auth_runtime','webdock_registry_runtime') OR s->>'kind' IS DISTINCT FROM 'session' THEN RETURN NULL; END IF;
 SELECT u.id,u.email,u.role,u."twoFactorEnabled" INTO actor_row FROM webdock_auth."user" u JOIN webdock_auth.session a ON a."userId"=u.id WHERE u.id=s->>'subject' AND a.id=s->>'sessionID' AND a."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword" AND (u.role='user' OR (u.role='operator' AND u."twoFactorEnabled"));
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF NOT webdock_rls.native_identity_authority(actor_row.id) THEN RETURN NULL; END IF;
 SELECT p.session_id,p.actor_id,p.customer_id,p.organization_id,p.expires_at,p.expired_at, (p.expired_at IS NULL AND p.expires_at>statement_timestamp() AND c.status='active' AND t.organization_id=p.organization_id) AS valid INTO preview_row FROM webdock_auth.studio_tenant_preview p LEFT JOIN webdock_admin.customers c ON c.id=p.customer_id LEFT JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id WHERE p.session_id=s->>'sessionID' AND p.actor_id=actor_row.id;
 RETURN jsonb_build_object('subject',actor_row.id,'email',actor_row.email,'sessionID',s->>'sessionID','operator',actor_row.role='operator' AND NOT FOUND,'root',actor_row.role='operator','preview',CASE WHEN FOUND THEN to_jsonb(preview_row) ELSE NULL END);
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.operator() RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT coalesce((webdock_rls.actor()->>'operator')::boolean,false) $$;
CREATE OR REPLACE FUNCTION webdock_rls.subject() RETURNS text LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT webdock_rls.actor()->>'subject' $$;
CREATE OR REPLACE FUNCTION webdock_rls.portal_user() RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT CASE WHEN count(*)=1 THEN jsonb_agg(jsonb_build_object('id',u.id,'name',u.name,'email',u.email,'role','operator','authSubject',u.auth_subject,'createdAt',u.created_at,'updatedAt',u.updated_at))->0 ELSE NULL END
 FROM webdock_admin.users u WHERE u.auth_subject=webdock_rls.subject() AND u.role='operator' AND coalesce((webdock_rls.actor()->>'root')::boolean,false)
$$;
CREATE OR REPLACE FUNCTION webdock_rls.worker(purposes text[]) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT coalesce(session_user='webdock_worker_runtime' AND webdock_rls.request_scope()->>'kind'='system' AND webdock_rls.request_scope()->>'purpose'=ANY(purposes),false) $$;
CREATE OR REPLACE FUNCTION webdock_rls.worker(purposes text[], cluster text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT coalesce(cluster IS NOT NULL AND webdock_rls.worker(purposes) AND webdock_rls.request_scope()->>'clusterID'=cluster,false) $$;
CREATE OR REPLACE FUNCTION webdock_rls.customer_allowed(customer text, writing boolean DEFAULT false) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE a jsonb;
BEGIN
 IF webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN webdock_rls.setup_allowed(customer); END IF;
 IF webdock_rls.worker(ARRAY['database-order']) THEN RETURN customer IS NOT NULL AND webdock_rls.request_scope()->>'customerID'=customer; END IF;
 a:=webdock_rls.actor(); IF a IS NULL THEN RETURN false; END IF;
 IF (a->>'operator')::boolean THEN RETURN true; END IF;
 IF a->'preview'<>'null'::jsonb THEN RETURN NOT writing AND coalesce((a->'preview'->>'valid')::boolean,false) AND a->'preview'->>'customer_id'=customer; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id JOIN webdock_auth.member m ON m."organizationId"=t.organization_id WHERE t.customer_id=customer AND c.status='active' AND m."userId"=a->>'subject' AND (NOT writing OR m.role IN ('owner','admin')));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.customer_permission(customer text, permission text, allow_operator boolean DEFAULT true) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT webdock_rls.customer_allowed(customer,true)
$$;
CREATE OR REPLACE FUNCTION webdock_rls.project_permission(project text, permission text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_admin.projects p WHERE p.id=project AND webdock_rls.customer_permission(p.customer_id,permission))
$$;
CREATE OR REPLACE FUNCTION webdock_rls.organization_allowed(org text, writing boolean DEFAULT false) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE a jsonb; customer text;
BEGIN
 a:=webdock_rls.actor(); IF a IS NULL THEN RETURN false; END IF;
 IF (a->>'operator')::boolean THEN RETURN true; END IF;
 IF NOT (a ? 'clerk') AND coalesce(webdock_rls.request_scope()->>'authProvider','')<>'clerk' AND NOT webdock_rls.native_organization_authority(org) THEN RETURN false; END IF;
 SELECT customer_id INTO customer FROM webdock_auth.tenant_customer WHERE organization_id=org;
 IF FOUND THEN RETURN webdock_rls.customer_allowed(customer,writing); END IF;
 RETURN a->'preview'='null'::jsonb AND EXISTS(SELECT 1 FROM webdock_auth.member m WHERE m."organizationId"=org AND m."userId"=a->>'subject' AND (NOT writing OR m.role IN ('owner','admin')));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.project_allowed(project text, writing boolean DEFAULT false) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT EXISTS(SELECT 1 FROM webdock_admin.projects p WHERE p.id=project AND webdock_rls.customer_allowed(p.customer_id,writing)) $$;
CREATE OR REPLACE FUNCTION webdock_rls.project_matches(project text,customer text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT EXISTS(SELECT 1 FROM webdock_admin.projects p WHERE p.id=project AND p.customer_id=customer) $$;
CREATE OR REPLACE FUNCTION webdock_rls.user_visible(target text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT webdock_rls.operator() OR target=webdock_rls.subject() OR EXISTS(SELECT 1 FROM webdock_auth.member m WHERE m."userId"=target AND webdock_rls.organization_allowed(m."organizationId")) $$;
CREATE OR REPLACE FUNCTION webdock_rls.agent_customer(customer text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT webdock_rls.worker(ARRAY['agent']) AND (EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.customer_id=customer AND p.cluster_id=webdock_rls.request_scope()->>'clusterID') OR EXISTS(SELECT 1 FROM webdock_mail.cluster_reservation r WHERE r.customer_id=customer AND r.cluster_id=webdock_rls.request_scope()->>'clusterID')) $$;
CREATE OR REPLACE FUNCTION webdock_rls.agent_project(project text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id=project AND webdock_rls.worker(ARRAY['agent'],p.cluster_id)) $$;
CREATE OR REPLACE FUNCTION webdock_rls.agent_backend(backend text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$ SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backend b WHERE b.id=backend AND webdock_rls.worker(ARRAY['agent'],b.cluster_id)) $$;
CREATE OR REPLACE FUNCTION webdock_rls.backend_owned(backend text, writing boolean DEFAULT false) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backend b WHERE b.id=backend AND (NOT writing OR b.kind='dedicated') AND (
 EXISTS(SELECT 1 FROM webdock_auth.hosting_database d WHERE (d.backend_id=b.id OR d.pending_backend_id=b.id) AND (CASE WHEN writing AND NOT webdock_rls.worker(ARRAY['database-order']) THEN webdock_rls.customer_permission(d.customer_id,'org:databases:manage') ELSE webdock_rls.customer_allowed(d.customer_id) END)) OR
 EXISTS(SELECT 1 FROM webdock_auth.hosting_database_migration m JOIN webdock_auth.hosting_database d ON d.id=m.database_id WHERE (m.source_backend_id=b.id OR m.target_backend_id=b.id) AND (CASE WHEN writing AND NOT webdock_rls.worker(ARRAY['database-order']) THEN webdock_rls.customer_permission(d.customer_id,'org:databases:manage') ELSE webdock_rls.customer_allowed(d.customer_id) END))))
$$;
CREATE OR REPLACE FUNCTION webdock_rls.plan_owned(plan text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_order o WHERE o.plan_revision_id=plan AND webdock_rls.customer_allowed(o.customer_id))
$$;
CREATE OR REPLACE FUNCTION webdock_rls.mail_key(customer text,connection text DEFAULT NULL,writing boolean DEFAULT false,management boolean DEFAULT false) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT session_user='webdock_auth_runtime' AND webdock_rls.request_scope()->>'kind'='mail-key' AND EXISTS(SELECT 1 FROM webdock_auth.mail_api_key k JOIN webdock_admin.customers c ON c.id=k.customer_id AND c.status='active' JOIN webdock_auth.mail_connection m ON m.id=k.connection_id AND m.customer_id=k.customer_id AND m.deleted_at IS NULL WHERE k.id=webdock_rls.request_scope()->>'keyID' AND k.customer_id=customer AND k.customer_id=webdock_rls.request_scope()->>'customerID' AND k.connection_id=webdock_rls.request_scope()->>'connectionID' AND (connection IS NULL OR k.connection_id=connection) AND k.revoked_at IS NULL AND (NOT management OR k.permission='full_access'))
$$;
`;
const signatures = [
  "request_scope()",
  "actor()",
  "native_identity_authority(text)",
  "native_organization_authority(text)",
  "operator()",
  "subject()",
  "portal_user()",
  "worker(text[])",
  "worker(text[],text)",
  "customer_allowed(text,boolean)",
  "customer_permission(text,text,boolean)",
  "project_permission(text,text)",
  "organization_allowed(text,boolean)",
  "project_allowed(text,boolean)",
  "project_matches(text,text)",
  "user_visible(text)",
  "agent_customer(text)",
  "agent_project(text)",
  "agent_backend(text)",
  "mail_key(text,text,boolean,boolean)",
  "backend_owned(text,boolean)",
  "plan_owned(text)",
];

/** A table needs an explicit classification; merely having customer_id never grants access. */
export function buildDatabaseRls(tables: readonly RlsTable[]): RlsPlan {
  const sql: string[] = [
    "GRANT USAGE ON SCHEMA webdock_auth,webdock_admin,webdock_mail,webdock_rls TO webdock_auth_runtime,webdock_worker_runtime,webdock_policy_owner,webdock_platform_owner;",
    "GRANT USAGE ON SCHEMA webdock_auth,webdock_admin TO webdock_auth_bootstrap;",
    "GRANT USAGE ON SCHEMA webdock_admin TO webdock_admin_runtime,webdock_registry_runtime; GRANT USAGE ON SCHEMA webdock_rls TO webdock_registry_runtime,webdock_auth_bootstrap;",
    "GRANT USAGE ON SCHEMA webdock TO webdock_runtime,webdock_browser_website;",
  ];
  const unknownTables: string[] = [];
  const classifiedTables: string[] = [];
  const seen = new Set<string>();
  for (const table of tables) {
    if (
      ![
        "public",
        "webdock",
        "webdock_admin",
        "webdock_auth",
        "webdock_mail",
      ].includes(table.schema)
    )
      throw Error("Out-of-scope schema: " + table.schema);
    const key = table.schema + "." + table.name;
    if (seen.has(key)) throw Error("Duplicate table " + key);
    seen.add(key);
  }
  for (const table of tables) {
    const key = table.schema + "." + table.name,
      q = ident(table.schema) + "." + ident(table.name);
    sql.push(
      `ALTER TABLE ${q} OWNER TO webdock_platform_owner; ALTER TABLE ${q} ENABLE ROW LEVEL SECURITY; ALTER TABLE ${q} FORCE ROW LEVEL SECURITY;`,
    );
    sql.push(
      `DO $policies$ DECLARE p record; BEGIN FOR p IN SELECT polname FROM pg_catalog.pg_policy WHERE polrelid=${literal(q)}::regclass LOOP EXECUTE format('DROP POLICY %I ON %s',p.polname,${literal(q)}); END LOOP; END $policies$;`,
    );
    sql.push(
      `REVOKE ALL ON ${q} FROM PUBLIC,${runtimeRoles.join(",")},webdock_policy_owner;`,
    );
    sql.push(
      `CREATE POLICY platform_owner ON ${q} TO webdock_platform_owner USING(true) WITH CHECK(true);`,
    );
    sql.push(`DROP TRIGGER IF EXISTS aa_webdock_rls_old_write ON ${q};`);
    for (const [role, map] of [
      ["webdock_policy_owner", metadata],
      ["webdock_auth_bootstrap", bootstrapMetadata],
    ] as const) {
      const cols = map[key];
      if (!cols) continue;
      const missing = cols.filter((c) => !table.columns.includes(c));
      if (missing.length)
        throw Error(
          "Missing metadata columns on " + key + ": " + missing.join(","),
        );
      sql.push(
        `GRANT SELECT(${cols.map(ident).join(",")}) ON ${q} TO ${role}; CREATE POLICY ${role === "webdock_policy_owner" ? "policy_metadata" : "bootstrap_metadata"} ON ${q} FOR SELECT TO ${role} USING(true);`,
      );
    }
  }
  const clerkReady = seen.has("webdock_auth.clerk_identity");
  let identityHelpers = helperSQL;
  if (clerkReady) {
    identityHelpers = identityHelpers.replace(
      ' SELECT u.id,u.email,u.role,u."twoFactorEnabled" INTO actor_row',
      clerkActorSQL + clerkLegacyActorSQL +
        ' SELECT u.id,u.email,u.role,u."twoFactorEnabled" INTO actor_row',
    );
    identityHelpers = identityHelpers.replace(
      " RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id JOIN webdock_auth.member",
      clerkCustomerSQL +
        " RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id JOIN webdock_auth.member",
    );
  }
  if (clerkReady) {
    identityHelpers = identityHelpers.replace(" RETURN a->'preview'='null'::jsonb AND EXISTS(SELECT 1 FROM webdock_auth.member", " IF a ? 'clerk' OR webdock_rls.request_scope()->>'authProvider'='clerk' THEN RETURN false; END IF;\n RETURN a->'preview'='null'::jsonb AND EXISTS(SELECT 1 FROM webdock_auth.member");
    identityHelpers = identityHelpers.replace("END $$;\nCREATE OR REPLACE FUNCTION webdock_rls.operator()", () => "EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value THEN RETURN NULL;\nEND $$;\nCREATE OR REPLACE FUNCTION webdock_rls.operator()");
    identityHelpers = identityHelpers.replace("AND u.role='operator' AND coalesce((webdock_rls.actor()->>'root')::boolean,false)", "AND (webdock_rls.request_scope() ? 'clerk' OR webdock_rls.request_scope()->>'authProvider'='clerk' OR u.role='operator') AND coalesce((webdock_rls.actor()->>'root')::boolean,false)");
  }
  if (clerkReady)
    identityHelpers = identityHelpers.replace(
      `JOIN webdock_auth.member m ON m."organizationId"=t.organization_id WHERE t.customer_id=customer AND c.status='active' AND m."userId"=a->>'subject' AND (NOT writing OR m.role IN ('owner','admin'))`,
      `WHERE t.customer_id=customer AND c.status='active' AND webdock_rls.membership_role(t.organization_id) IS NOT NULL AND (NOT writing OR webdock_rls.membership_role(t.organization_id) IN ('owner','admin'))`,
    );
  identityHelpers = identityHelpers.replace(
    " RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer",
    " IF NOT (a ? 'clerk') AND coalesce(webdock_rls.request_scope()->>'authProvider','')<>'clerk' AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=customer AND NOT webdock_rls.native_organization_authority(organization_id)) THEN RETURN false; END IF;\n RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer",
  );
  sql.push(identityHelpers, gitInvalidationSQL);
  if (clerkReady)
    sql.push(
      clerkMembershipSQL,
      clerkPermissionSQL,
      clerkBillingSQL,
      clerkBillingReadSQL,
      clerkProjectionSQL,
      setupClerkRevalidationSQL,
      setupLegacyRevalidationSQL,
    );
  sql.push(
    clerkReady
      ? setupHelperSQL
          .replace(
            " RETURN EXISTS(\n SELECT 1 FROM webdock_auth.hosting_setup s",
            clerkSetupSQL +
              " RETURN EXISTS(\n SELECT 1 FROM webdock_auth.hosting_setup s",
          )
          .replace(
            "((u.role='operator' AND u.\"twoFactorEnabled\") OR (u.role='user' AND m.role IN ('owner','admin') AND h.mode='selfservice'))",
            "(CASE WHEN webdock_rls.request_scope()->>'authProvider'='clerk' THEN webdock_rls.setup_legacy_permission(customer,'operator') OR (h.mode='selfservice' AND webdock_rls.setup_legacy_permission(customer,'org:projects:manage') AND (coalesce(s.configuration->'database'->>'mode','none')='none' OR webdock_rls.setup_legacy_permission(customer,'org:databases:manage')) AND (coalesce(s.configuration->'source'->>'kind','template')<>'git' OR webdock_rls.setup_legacy_permission(customer,'org:deployments:create'))) ELSE ((u.role='operator' AND u.\"twoFactorEnabled\") OR (u.role='user' AND m.role IN ('owner','admin') AND h.mode='selfservice')) END)",
          )
          .replace(
            "IF proof IS NULL THEN RETURN true; END IF;",
            "IF proof IS NULL THEN RETURN CASE WHEN webdock_rls.request_scope()->>'authProvider'='clerk' THEN webdock_rls.setup_legacy_permission(customer,permission) ELSE true END; END IF;",
          )
      : setupHelperSQL,
  );
  for (const signature of signatures)
    sql.push(
      `ALTER FUNCTION webdock_rls.${signature} OWNER TO webdock_policy_owner; REVOKE ALL ON FUNCTION webdock_rls.${signature} FROM PUBLIC; GRANT EXECUTE ON FUNCTION webdock_rls.${signature} TO webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime,webdock_platform_owner;`,
    );
  sql.push("GRANT SELECT ON webdock_rls.context_key TO webdock_policy_owner;");
  sql.push(
    "DO $$ BEGIN IF to_regprocedure('webdock_admin.lock_hosting_project(character varying)') IS NOT NULL THEN GRANT EXECUTE ON FUNCTION webdock_admin.lock_hosting_project(varchar) TO webdock_auth_runtime,webdock_worker_runtime; END IF; END $$;",
  );
  if(seen.has("webdock_auth.plugin_invocation"))sql.push(pluginScopeSQL,pluginInvocationFenceSQL);
  if(seen.has("webdock_mail.provider_binding"))sql.push(mailPluginPolicySQL);
  if(seen.has("webdock_auth.hosting_plugin_provisioning") && seen.has("webdock_auth.plugin_invocation"))sql.push(pluginProvisioningRlsSQL,pluginTemplateSchemaSQL);
  const worker = (purposes: string[], cluster?: string) =>
    `webdock_rls.worker(ARRAY[${purposes.map(literal).join(",")}]::text[]${cluster ? "," + cluster : ""})`;
  for (const t of tables) {
    const q = ident(t.schema) + "." + ident(t.name),
      key = t.schema + "." + t.name;
    const col = (name: string) => {
      if (!t.columns.includes(name))
        throw Error("Missing policy column " + key + "." + name);
      return `${ident(t.name)}.${ident(name)}::text`;
    };
    const oldWrite = new Map<string, string>([
      ["webdock_platform_owner", "true"],
    ]);
    const policy = (
      role: string,
      read: string,
      write: string,
      grant = true,
      updateWrite = write,
      deleteWrite = updateWrite,
    ) => {
      oldWrite.set(role, updateWrite);
      if (grant)
        sql.push(`GRANT SELECT,INSERT,UPDATE,DELETE ON ${q} TO ${role};`);
      sql.push(
        `CREATE POLICY ${role}_read ON ${q} FOR SELECT TO ${role} USING(${read}); CREATE POLICY ${role}_insert ON ${q} FOR INSERT TO ${role} WITH CHECK(${write}); CREATE POLICY ${role}_update ON ${q} FOR UPDATE TO ${role} USING(${read}) WITH CHECK(${updateWrite}); CREATE POLICY ${role}_delete ON ${q} FOR DELETE TO ${role} USING(${deleteWrite});`,
      );
    };
    let read = "false",
      write = "false",
      workRead = "false",
      workWrite = "false",
      runtimeUpdate: string | undefined,
      runtimeDelete: string | undefined,
      workUpdate: string | undefined,
      workDelete: string | undefined,
      known = true;
    if (t.schema === "public") {
      classifiedTables.push(key);
      continue;
    }
    if (t.schema === "webdock") {
      if (websiteTables.has(t.name)) {
        policy("webdock_runtime", "true", "true");
        policy("webdock_browser_website", "true", "true");
      } else known = false;
      (known ? classifiedTables : unknownTables).push(key);
      continue;
    }
    if (t.schema === "webdock_admin") {
      if (!adminTables.has(t.name)) {
        unknownTables.push(key);
        continue;
      }
      policy("webdock_admin_runtime", "true", "true");
      policy(
        "webdock_registry_runtime",
        "webdock_rls.operator()",
        "webdock_rls.operator()",
        true,
        t.name === "audit_events" ? "false" : "webdock_rls.operator()",
        t.name === "audit_events" ? "false" : "webdock_rls.operator()",
      );
      if (t.name === "customers") {
        read = `webdock_rls.customer_allowed(${col("id")}) OR webdock_rls.actor()->'preview'->>'customer_id'=${col("id")} OR webdock_rls.mail_key(${col("id")})`;
        write = `webdock_rls.customer_allowed(${col("id")},true)`;
        workRead = `webdock_rls.agent_customer(${col("id")}) OR ${worker(["git-worker", "mail-cron", "mail-send-gateway", "mail-webmail-gateway", "database-gateway"])}`;
      } else if (t.name === "projects") {
        read = `webdock_rls.customer_allowed(${col("customer_id")})`;
        write = "webdock_rls.operator()";
        workRead = `webdock_rls.agent_project(${col("id")}) OR ${worker(["git-worker", "database-gateway", "mail-send-gateway", "mail-webmail-gateway"])}`;
      } else {
        read = "webdock_rls.operator()";
        write = "webdock_rls.operator()";
      }
      if (t.name === "customers")
        workRead += ` OR (${worker(["database-order"])} AND webdock_rls.customer_allowed(${col("id")}))`;
      if (t.name === "projects")
        workRead += ` OR (${worker(["database-order"])} AND webdock_rls.customer_allowed(${col("customer_id")}))`;
    } else if (t.schema === "webdock_mail") {
      if (!mailCustomer.has(t.name)) {
        unknownTables.push(key);
        continue;
      }
      read = `webdock_rls.customer_allowed(${col("customer_id")})`;
      write = `webdock_rls.customer_allowed(${col("customer_id")},true)`;
      if (
        [
          "identity",
          "webmail_launch",
          "webmail_session",
          "webmail_write",
        ].includes(t.name)
      )
        read = `(${read}) AND (webdock_rls.customer_allowed(${col("customer_id")},true) OR ${col("subject")}=webdock_rls.subject())`;
      if (t.name === "service")
        read += ` OR webdock_rls.mail_key(${col("customer_id")})`;
      workRead =
        workWrite = `${worker(["mail-send-gateway", "mail-webmail-gateway"])} OR ${t.columns.includes("cluster_id") ? worker(["agent"], col("cluster_id")) : `EXISTS(SELECT 1 FROM webdock_mail.cluster_reservation r WHERE r.customer_id::text=${col("customer_id")} AND ${worker(["agent"], "r.cluster_id::text")})`}`;
      if (t.name === "service") workRead += ` OR ${worker(["mail-cron"])}`;
      if (t.name === "provider_lease") {
        read = write = runtimeUpdate = runtimeDelete = "false";
        workRead = workUpdate = worker(["mail-send-gateway"]);
        workWrite = `(${workRead}) AND ${col("attempted_at")} IS NULL AND ${col("revoked_at")} IS NULL`;
        workDelete = "false";
      }
      if (t.name === "provider_binding") {
        // Selection is an explicit tenant admin decision; the sender only reads its grant.
        workRead = worker(["mail-send-gateway"]);
        workWrite = workUpdate = workDelete = runtimeDelete = "false";
      }
      if (["month_usage", "budget_reservation"].includes(t.name)) {
        // Usage ledgers are observable by the tenant but only authenticated send
        // gateways may reserve or settle recipient units.
        write = "false";
        workRead = workWrite = worker([
          "mail-send-gateway",
          "mail-webmail-gateway",
        ]);
        workDelete = "false";
      }
    } else if (["clerk_identity", "clerk_organization"].includes(t.name)) {
      policy("webdock_auth_bootstrap", "true", "false");
      sql.push(`GRANT SELECT ON ${q} TO webdock_auth_bootstrap;`);
      read = "webdock_rls.operator()";
      write =
        t.name === "clerk_organization"
          ? `webdock_rls.operator() AND ${col("issuer")}=webdock_rls.actor()->'clerk'->>'issuer' AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.organization_id=${col("organization_id")} AND c.status='active')`
          : "false";
      runtimeUpdate = runtimeDelete = "false";
      workRead =
        t.name === "clerk_identity"
          ? seen.has("webdock_mail.identity")
            ? `EXISTS(SELECT 1 FROM webdock_mail.identity i WHERE i.subject=${col("user_id")} AND (${worker(["mail-send-gateway", "mail-webmail-gateway"])} OR webdock_rls.agent_customer(i.customer_id)))`
            : "false"
          : seen.has("webdock_mail.service")
            ? `EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_mail.service s ON s.customer_id=t.customer_id WHERE t.organization_id=${col("organization_id")} AND (${worker(["mail-send-gateway", "mail-webmail-gateway"])} OR webdock_rls.agent_customer(t.customer_id)))`
            : "false";
    } else if (t.schema === "webdock_auth" && t.name === "hosting_database_binding_route") {
      // Explicit operator-reviewed transport authority; an agent may only inspect its own source routes.
      read = write = "webdock_rls.operator()";
      runtimeDelete = "false";
      workRead = worker(["agent"], col("source_cluster_id"));
      workWrite = workUpdate = workDelete = "false";
    } else if (t.schema === "webdock_auth" && t.name === "repository_lease") {
      // Native repository callbacks have a separate fixed purpose, never ordinary Invoke.
      read = write = "false";
      workRead = workWrite = worker(["repository-admission"]);
    } else if (t.schema === "webdock_auth" && t.name === "agent_transport_enrollment") {
      // Enrollment is an explicit offline authority change. Runtime can only record liveness.
      policy("webdock_auth_bootstrap", "true", "false", false, "true", "false");
      sql.push(`GRANT SELECT,UPDATE(last_sequence,last_seen) ON ${q} TO webdock_auth_bootstrap;`);
      read = write = workRead = workWrite = "false";
    } else if (t.schema === "webdock_auth" && t.name === "agent_runtime_dispatch") {
      policy("webdock_auth_bootstrap", "true", "true", false, "true", "false");
      sql.push(`GRANT SELECT,INSERT,UPDATE ON ${q} TO webdock_auth_bootstrap;`);
      read = write = workRead = workWrite = "false";
    } else if (t.schema === "webdock_auth" && ["access_invitation_state", "access_delivery_job"].includes(t.name)) {
      policy("webdock_auth_bootstrap", "true", "true");
      sql.push(`GRANT SELECT,INSERT,UPDATE,DELETE ON ${q} TO webdock_auth_bootstrap;`);
      read = `webdock_rls.organization_allowed(${col("organization_id")},true)`;
      write = read + (t.name === "access_delivery_job" ? ` AND ${col("actor_id")}=webdock_rls.subject()` : ` AND EXISTS(SELECT 1 FROM webdock_auth.invitation i WHERE i.id=${col("invitation_id")} AND i."organizationId"=${col("organization_id")})`);
      runtimeDelete = workRead = workWrite = "false";
    } else if (t.schema === "webdock_auth" && ["local_identity_activation", "local_organization_activation"].includes(t.name)) {
      policy("webdock_auth_bootstrap", "true", "false", false, "false", "false");
      const subject = t.name === "local_identity_activation" ? "user_id" : "organization_id";
      sql.push(`GRANT SELECT(${ident(subject)},bindings) ON ${q} TO webdock_auth_bootstrap;`);
      read = write = workRead = workWrite = "false";
    } else if (identity.has(t.name)) {
      policy("webdock_auth_bootstrap", "true", "true");
      if (t.name === "user") read = `webdock_rls.user_visible(${col("id")})`;
      else if (t.name === "session")
        read = `${col("userId")}=webdock_rls.subject() OR webdock_rls.operator()`;
      else if (t.name === "organization")
        read = `webdock_rls.organization_allowed(${col("id")})`;
      else if (t.name === "member")
        read = `webdock_rls.organization_allowed(${col("organizationId")})`;
      else if (t.name === "invitation")
        read = `webdock_rls.organization_allowed(${col("organizationId")}) OR lower(${col("email")})=lower(webdock_rls.actor()->>'email')`;
      else if (t.name === "oauthClient")
        read = `webdock_rls.operator() OR EXISTS(SELECT 1 FROM webdock_auth.app_binding b WHERE b.client_id=${col("clientId")} AND webdock_rls.organization_allowed(b.organization_id))`;
      else read = "webdock_rls.operator()";
      if (["organization", "member", "invitation"].includes(t.name))
        write = `webdock_rls.organization_allowed(${col(t.name === "organization" ? "id" : "organizationId")},true)`;
      else write = "webdock_rls.operator()";
      // Authentication secrets never become tenant-readable merely through membership.
      if (
        ![
          "user",
          "session",
          "organization",
          "member",
          "invitation",
          "oauthClient",
        ].includes(t.name)
      ) {
        read = "false";
        write = "false";
      }
      if (t.name === "clerk_identity_mapping")
        workRead = `${worker(["mail-cron", "mail-send-gateway", "mail-webmail-gateway", "git-worker"])} OR (${worker(["database-order"])} AND ${col("local_subject")}=webdock_rls.request_scope()->>'subject')`;
      if (t.name === "clerk_session_mapping")
        workRead = `EXISTS(SELECT 1 FROM webdock_auth.clerk_identity_mapping m WHERE m.issuer=${col("issuer")} AND m.external_subject=${col("external_subject")} AND m.revoked_at IS NULL AND (
          (${worker(["git-worker"])} AND EXISTS(SELECT 1 FROM webdock_auth.git_release r WHERE r.approved_by=m.local_subject AND r.approved_session_id=${col("local_session_id")} AND r.status='deploying' AND r.lease_until>now()))
          OR (${worker(["database-order"])} AND m.local_subject=webdock_rls.request_scope()->>'subject' AND ${col("local_session_id")}=webdock_rls.request_scope()->>'sessionID')
        ))`;
      if (["user", "session", "member", "organization"].includes(t.name))
        workRead = worker([
          "database-order",
          "agent",
          "git-worker",
          "mail-cron",
          "mail-send-gateway",
          "mail-webmail-gateway",
          "database-gateway",
        ]);
    } else if (authCustomer.has(t.name)) {
      read = `webdock_rls.customer_allowed(${col("customer_id")})`;
      write = `webdock_rls.customer_allowed(${col("customer_id")},true)`;
      const domains = t.name.startsWith("hosting_database")
        ? ["org:databases:manage"]
        : ["hosting_operation", "hosting_reservation"].includes(t.name)
          ? [
              "org:projects:manage",
              "org:deployments:create",
              "org:databases:manage",
            ]
          : ["git_build", "git_artifact", "git_release"].includes(t.name)
            ? ["org:deployments:create"]
            : t.name.startsWith("git_") ||
                [
                  "hosting_setup",
                  "hosting_project",
                  "hosting_service_binding",
                  "hosting_vercel",
                  "hosting_vercel_flow",
                ].includes(t.name)
              ? ["org:projects:manage"]
              : ["plan_offer", "tenant_subscription", "subscription_history"].includes(t.name)
                ? ["org:billing:manage"]
                : [];
      if (domains.length)
        write =
          "(" +
          domains
            .map(
              (permission) =>
                `webdock_rls.customer_permission(${col("customer_id")},'${permission}')`,
            )
            .join(" OR ") +
          ")";
      if (t.columns.includes("project_id"))
        write += ` AND (${col("project_id")} IS NULL OR webdock_rls.project_matches(${col("project_id")},${col("customer_id")}))`;
      if (t.name === "git_source") {
        runtimeUpdate = `(${write}) OR (webdock_rls.customer_permission(${col("customer_id")},'org:deployments:create') AND webdock_rls.project_matches(${col("project_id")},${col("customer_id")}))`;
        runtimeDelete = write;
      }
      if (t.name === "tenant_customer") {
        read += ` OR webdock_rls.actor()->'preview'->>'customer_id'=${col("customer_id")}`;
        write = "webdock_rls.operator()";
      }
      if (t.name.startsWith("mail_")) {
        const connection = t.columns.includes("connection_id")
          ? col("connection_id")
          : t.name === "mail_connection"
            ? col("id")
            : "NULL";
        const management = ![
          "mail_message",
          "mail_message_content",
          "mail_recipient",
          "mail_batch",
          "mail_batch_item",
          "mail_idempotency",
          "mail_usage_period",
          "mail_usage_reservation",
          "mail_rate_window",
          "mail_request_log",
        ].includes(t.name);
        read += ` OR webdock_rls.mail_key(${col("customer_id")},${connection})`;
        write += ` OR webdock_rls.mail_key(${col("customer_id")},${connection},true,${management})`;
        const sessionSender = `(webdock_rls.customer_allowed(${col("customer_id")}) AND webdock_rls.actor()->'preview'='null'::jsonb)`;
        if (!management) write += ` OR ${sessionSender}`;
        if (["mail_event", "mail_webhook_delivery"].includes(t.name)) {
          runtimeUpdate = runtimeDelete = write;
          write += ` OR webdock_rls.mail_key(${col("customer_id")},${connection}) OR ${sessionSender}`;
        }
        workRead = workWrite = worker(["mail-cron", "mail-send-gateway"]);
        if (t.name === "mail_sender_domain")
          workRead += ` OR webdock_rls.agent_customer(${col("customer_id")})`;
      } else if (t.name.startsWith("git_"))
        workRead = workWrite = worker(
          t.name === "git_connection"
            ? ["git-worker", "git-webhook"]
            : ["git-worker"],
        );
      else if (t.name === "tenant_storage")
        workRead = workWrite = worker(["storage-cron"]);
      else if (
        t.name === "tenant_customer" ||
        [
          "tenant_subscription",
          "subscription_history",
          "hosting_byok_policy",
          "hosting_limit",
        ].includes(t.name)
      )
        workRead = worker([
          "git-worker",
          "mail-cron",
          "mail-send-gateway",
          "mail-webmail-gateway",
          "database-gateway",
        ]);
      if (
        (t.name.startsWith("hosting_") && t.name !== "hosting_setup") ||
        t.name === "tenant_customer"
      ) {
        const agent = t.columns.includes("cluster_id")
          ? worker(["agent"], col("cluster_id"))
          : t.name === "hosting_database"
            ? `webdock_rls.agent_backend(${col("backend_id")})`
            : t.columns.includes("project_id")
              ? `webdock_rls.agent_project(${col("project_id")})`
              : `webdock_rls.agent_customer(${col("customer_id")})`;
        workRead += ` OR ${agent}`;
        if (t.name !== "tenant_customer") workWrite += ` OR ${agent}`;
        if (
          [
            "hosting_project",
            "hosting_service_binding",
            "hosting_app",
            "hosting_operation",
            "hosting_reservation",
            "hosting_vercel",
            "hosting_byok_policy",
          ].includes(t.name)
        ) {
          workRead += ` OR ${worker(["git-worker"])}`;
          workWrite += ` OR ${worker(["git-worker"])}`;
        }
      }
      if (
        [
          "tenant_customer",
          "hosting_project",
          "hosting_database",
          "hosting_database_offer",
          "hosting_database_order",
          "hosting_operation",
        ].includes(t.name)
      ) {
        const order = `(${worker(["database-order"])} AND webdock_rls.customer_allowed(${col("customer_id")})${t.columns.includes("project_id") ? ` AND (${col("project_id")} IS NULL OR webdock_rls.project_matches(${col("project_id")},${col("customer_id")}))` : ""}${t.columns.includes("cluster_id") ? ` AND (${col("cluster_id")} IS NULL OR ${worker(["database-order"], col("cluster_id"))})` : ""})`;
        workRead += ` OR ${order}`;
        if (t.name !== "tenant_customer" && t.name !== "hosting_project")
          workWrite += ` OR ${order}`;
      }
      if (
        [
          "hosting_byok_policy",
          "hosting_turbosmtp",
          "tenant_subscription",
        ].includes(t.name)
      )
        read += ` OR webdock_rls.mail_key(${col("customer_id")})`;
      if (["hosting_database", "hosting_project"].includes(t.name))
        workRead += ` OR ${worker(["database-gateway"])}`;
      if (t.name === "hosting_turbosmtp")
        workRead += ` OR ${worker(["mail-cron", "mail-send-gateway"])}`;
    } else if (t.name === "app_binding" || t.name === "project_grant") {
      read = `webdock_rls.organization_allowed(${col("organization_id")})`;
      write =
        t.name === "app_binding"
          ? "webdock_rls.operator()"
          : `webdock_rls.organization_allowed(${col("organization_id")},true) AND EXISTS(SELECT 1 FROM webdock_auth.app_binding b WHERE b.id::text=${col("binding_id")} AND b.organization_id::text=${col("organization_id")})`;
    } else if (
      ["managed_database_launch", "managed_database_session"].includes(t.name)
    ) {
      const sessionColumn = t.columns.includes("clerk_identity")
        ? `coalesce(${col("session_id")},(${col("clerk_identity")})::jsonb->>'sessionID')`
        : col("session_id");
      const own = `${col("subject")}=webdock_rls.subject() AND ${sessionColumn}=webdock_rls.actor()->>'sessionID' AND webdock_rls.actor()->'preview'='null'::jsonb`;
      const target = `EXISTS(SELECT 1 FROM webdock_auth.hosting_database d WHERE d.id::text=${col("database_id")} AND webdock_rls.customer_permission(d.customer_id,'org:databases:manage'))`;
      if (t.name === "managed_database_launch") {
        read = write = `(${own}) AND (${target})`;
        runtimeUpdate = runtimeDelete = "false";
      }
      workRead = worker(["database-gateway"]);
      workWrite = t.name === "managed_database_session" ? workRead : "false";
      workUpdate = t.name === "managed_database_launch" ? workRead : "false";
      workDelete = t.name === "managed_database_session" ? workRead : "false";
      if (t.columns.includes("clerk_identity")) {
        const reference = `((${col("clerk_identity")} IS NULL AND webdock_rls.actor()->'clerk' IS NULL) OR ((${col("clerk_identity")})::jsonb->>'issuer'=webdock_rls.actor()->'clerk'->>'issuer' AND (${col("clerk_identity")})::jsonb->>'userID'=webdock_rls.actor()->'clerk'->>'userID' AND ((${col("clerk_identity")})::jsonb->>'organizationID') IS NOT DISTINCT FROM (webdock_rls.actor()->'clerk'->>'clerkOrganizationID')))`;
        write = `(${write}) AND ${reference}`;
      }
    } else if (t.name === "database_binding") {
      read = `webdock_rls.project_allowed(${col("project_id")})`;
      write = "webdock_rls.operator()";
      workRead = worker(["database-gateway"]);
    } else if (childRelations[t.name]) {
      const relation = childRelations[t.name];
      const parent = ident("webdock_auth") + "." + ident(relation.parent);
      const inherited = `EXISTS(SELECT 1 FROM ${parent} parent WHERE parent.${ident(relation.key)}::text=${col(relation.foreign)})`;
      read = inherited;
      // Parent visibility alone must not authorize a member to mutate children.
      const target =
        relation.parent === "hosting_project"
          ? `webdock_rls.project_permission(${col(relation.foreign)},'org:projects:manage')`
          : relation.parent === "database_binding"
            ? `EXISTS(SELECT 1 FROM ${parent} parent WHERE parent.id::text=${col(relation.foreign)} AND webdock_rls.project_permission(parent.project_id,'org:databases:manage'))`
            : relation.parent === "database_grant"
              ? `EXISTS(SELECT 1 FROM ${parent} parent JOIN webdock_auth.database_binding b ON b.id=parent.binding_id WHERE parent.id::text=${col(relation.foreign)} AND (parent.subject=webdock_rls.subject() OR webdock_rls.project_permission(b.project_id,'org:databases:manage')))`
              : `EXISTS(SELECT 1 FROM ${parent} parent WHERE parent.${ident(relation.key)}::text=${col(relation.foreign)} AND webdock_rls.customer_permission(parent.customer_id,'${t.name.startsWith("hosting_database") ? "org:databases:manage" : "org:deployments:create"}'))`;
      write = target;
      if (
        ["database_grant", "database_runtime_owner", "database_audit"].includes(
          t.name,
        )
      ) {
        read = `(${inherited}) AND (${col("subject")}=webdock_rls.subject() OR webdock_rls.operator())`;
        write = t.name === "database_audit" ? read : "webdock_rls.operator()";
      }
      if (t.name === "database_launch") {
        const sessionColumn = t.columns.includes("clerk_identity")
          ? `coalesce(${col("session_id")},(${col("clerk_identity")})::jsonb->>'sessionID')`
          : col("session_id");
        write = `(${inherited}) AND ${sessionColumn}=webdock_rls.actor()->>'sessionID' AND webdock_rls.actor()->'preview'='null'::jsonb`;
      }
      if (t.name === "database_session") write = "false";
      if (t.name.startsWith("database_")) {
        workRead = `(${inherited}) AND ${worker(["database-gateway"])}`;
        workWrite = [
          "database_launch",
          "database_session",
          "database_audit",
        ].includes(t.name)
          ? workRead
          : "false";
      } else if (t.name === "hosting_app") {
        workRead =
          workWrite = `webdock_rls.agent_project(${col("project_id")}) OR ${worker(["git-worker"])}`;
      } else if (t.name === "git_check_outbox")
        workRead = workWrite = worker(["git-worker"]);
      else
        workRead =
          workWrite = `(${inherited}) AND ${worker(["agent", "database-order"])}`;
    } else if (
      ["studio_tenant_preview", "console_tenant_preview"].includes(t.name)
    ) {
      const own =
        `${col("session_id")}=webdock_rls.actor()->>'sessionID' AND ${col("actor_id")}=webdock_rls.subject()` +
        (t.name === "console_tenant_preview"
          ? ` AND ${col("issuer")}=webdock_rls.actor()->'clerk'->>'issuer'`
          : "");
      read = own;
      write = `(${own}) AND coalesce((webdock_rls.actor()->>'root')::boolean,false)`;
      workRead = worker([
        "database-order",
        "agent",
        "git-worker",
        "mail-cron",
        "mail-send-gateway",
        "mail-webmail-gateway",
        "database-gateway",
      ]);
    } else if (["hosting_plugin_provisioning","hosting_plugin_provisioning_request"].includes(t.name)) {
      const provision = seen.has("webdock_auth.plugin_invocation")
        ? pluginProvisioningPolicy(t.name)
        : { read: "false", write: "false", workerRead: "false", workerWrite: "false" };
      read=provision.read;write=provision.write;workRead=provision.workerRead;workWrite=provision.workerWrite;runtimeDelete=workDelete="false";
      if(t.name.endsWith("_request"))runtimeUpdate=workUpdate="false";
    } else if ((pluginTables as readonly string[]).includes(t.name)) {
      // Retained document tables can predate the invocation migration during offline upgrades.
      const plugin = ["plugin_document", "plugin_document_receipt"].includes(t.name) && !seen.has("webdock_auth.plugin_invocation")
        ? { read: "false", write: "false", workerRead: "false", workerWrite: "false" }
        : pluginPolicy(t.name);
      read=plugin.read;write=plugin.write;workRead=plugin.workerRead;workWrite=plugin.workerWrite;
      runtimeDelete=workDelete="false";
      if(t.name==="plugin_source_chunk"){runtimeDelete=write;workDelete=workWrite;}
      if(["plugin_document_receipt","plugin_release"].includes(t.name))runtimeUpdate=workUpdate="false";
    } else if ((billingTables as readonly string[]).includes(t.name)) {
      const billing = billingPolicy(t.name);
      read = billing.read;
      write = billing.write;
      runtimeUpdate = billing.updateWrite;
      workRead = billing.workerRead;
      workWrite = billing.workerWrite;
      runtimeDelete = workDelete = "false";
    } else if (t.name === "provider_event_receipt") {
      read = "webdock_rls.operator()";
      write = "false";
    } else if (t.name === "api_job") {
      read = "webdock_rls.operator()";
      write = "false";
      workRead = workWrite = worker(["api-scheduler"]);
      workDelete = "false";
    } else if (authGlobal.has(t.name)) {
      read = write = "webdock_rls.operator()";
      if (
        [
          "platform_settings",
          "plan_template",
          "hosting_database_plan",
        ].includes(t.name)
      )
        read = `webdock_rls.actor() IS NOT NULL OR webdock_rls.request_scope()->>'kind'='mail-key'`;
      if (["hosting_audit", "access_event"].includes(t.name)) {
        const subject = col(
          t.name === "hosting_audit" ? "subject" : "actor_id",
        );
        read += ` OR ${subject}=webdock_rls.subject()`;
        write += ` OR ${subject}=webdock_rls.subject()`;
        workRead = workWrite = worker([
          "agent",
          "agent-enrollment",
          "git-worker",
          "mail-cron",
          "mail-callback",
          "storage-cron",
          "mail-send-gateway",
          "mail-webmail-gateway",
          "database-gateway",
        ]);
      } else if (t.name.startsWith("hosting_")) {
        const cluster =
          t.name === "hosting_cluster"
            ? col("id")
            : t.columns.includes("cluster_id")
              ? col("cluster_id")
              : undefined;
        if (cluster)
          workRead = workWrite = worker(["agent", "agent-enrollment"], cluster);
        if (t.name === "hosting_rate")
          workRead = workWrite = worker([
            "agent",
            "agent-enrollment",
            "git-worker",
          ]);
        if (t.name === "hosting_database_plan") workRead = worker(["agent"]);
      } else if (t.name.startsWith("git_"))
        workRead = workWrite = worker(
          t.name === "git_receipt"
            ? ["git-worker", "git-webhook"]
            : ["git-worker"],
        );
      else if (
        t.name.startsWith("mail_") &&
        t.name !== "mail_product_migration"
      )
        workRead = workWrite = worker(
          t.name.startsWith("mail_callback_")
            ? ["mail-cron", "mail-callback"]
            : ["mail-cron"],
        );
      else if (t.name === "platform_settings")
        workRead = worker([
          "agent",
          "git-worker",
          "mail-cron",
          "mail-send-gateway",
          "mail-webmail-gateway",
        ]);
      if (t.name === "hosting_database_plan") {
        read = `webdock_rls.operator() OR (webdock_rls.actor() IS NOT NULL AND ${col("availability")}='public') OR webdock_rls.plan_owned(${col("id")})`;
        workRead += ` OR (${worker(["database-order"])} AND (${col("availability")}='public' OR webdock_rls.plan_owned(${col("id")}) OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_offer o WHERE o.plan_revision_id::text=${col("id")} AND webdock_rls.customer_allowed(o.customer_id))))`;
      }
      if (t.name === "hosting_database_backend") {
        workRead += ` OR ${worker(["database-gateway"])}`;
        read = `webdock_rls.operator() OR webdock_rls.backend_owned(${col("id")}) OR (webdock_rls.actor() IS NOT NULL AND ${col("kind")}='shared-pool' AND ${col("state")}='ready')`;
        write = `webdock_rls.operator() OR (${col("kind")}='dedicated' AND webdock_rls.backend_owned(${col("id")},true))`;
        const order = worker(["database-order"], col("cluster_id"));
        workRead += ` OR (${order} AND (${col("kind")}='shared-pool' AND ${col("state")}='ready' OR webdock_rls.backend_owned(${col("id")})))`;
        workUpdate = `${workWrite} OR (${order} AND ${col("kind")}='dedicated' AND webdock_rls.backend_owned(${col("id")},true))`;
        workDelete = workWrite;
        workWrite += ` OR (${order} AND ${col("kind")}='dedicated')`;
        // Newly inserted dedicated backends must be visible to RETURNING before the database child is inserted.
        workRead += ` OR (${order} AND ${col("kind")}='dedicated' AND ${col("state")}='provisioning')`;
      }
      if (t.name === "hosting_database_pool") {
        read += ` OR webdock_rls.actor() IS NOT NULL`;
        workRead += ` OR ${worker(["database-order"], col("cluster_id"))}`;
      }
      if (t.name === "hosting_database_port") {
        read += ` OR webdock_rls.backend_owned(${col("backend_id")})`;
        workRead += ` OR ${worker(["database-order"], col("cluster_id"))}`;
        workWrite += ` OR (${worker(["database-order"], col("cluster_id"))} AND EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backend b WHERE b.id::text=${col("backend_id")} AND b.cluster_id::text=${col("cluster_id")} AND b.kind='dedicated' AND b.state='provisioning'))`;
      }
      if (t.name === "hosting_audit") {
        const order = `(${worker(["database-order"])} AND EXISTS(SELECT 1 FROM webdock_auth.hosting_database d WHERE d.id::text=${col("target_id")} AND webdock_rls.customer_allowed(d.customer_id)))`;
        workRead += ` OR ${order}`;
        workWrite += ` OR ${order}`;
      }
      if (t.name === "snowflake_state") {
        // The shared ID allocator has no tenant records; invoker functions need its counter.
        read =
          write =
          workRead =
          workWrite =
            "webdock_rls.request_scope() IS NOT NULL";
        policy("webdock_auth_bootstrap", "true", "true", false);
        sql.push(`GRANT SELECT,UPDATE ON ${q} TO webdock_auth_bootstrap;`);
      }
    } else known = false;
    if (known) {
      const setup = setupPolicyFor(t);
      if (setup) {
        const previousWrite = workWrite;
        workRead += ` OR (${setup.read})`;
        workWrite += ` OR (${setup.write})`;
        workDelete ??= workUpdate ?? previousWrite;
        workUpdate = `(${workUpdate ?? previousWrite}) OR (${setup.update ?? setup.write})`;
      }
      if (t.name === "hosting_setup" && seen.has("webdock_auth.hosting_plugin_provisioning"))
        workRead += ` OR (webdock_rls.agent_project(${col("project_id")}) AND EXISTS(SELECT 1 FROM webdock_auth.hosting_plugin_provisioning provision WHERE provision.setup_id=${col("id")}))`;
      if (modulePreflightTables.has(key))
        sql.push(
          `GRANT SELECT ON ${q} TO webdock_worker_runtime; CREATE POLICY installation_preflight_read ON ${q} FOR SELECT TO webdock_worker_runtime USING(${worker(["installation-preflight"])});`,
        );
      classifiedTables.push(key);
      policy(
        "webdock_auth_runtime",
        read,
        write,
        true,
        runtimeUpdate,
        runtimeDelete,
      );
      policy(
        "webdock_worker_runtime",
        workRead,
        workWrite,
        t.name !== "hosting_setup",
        workUpdate,
        workDelete,
      );
      if (t.schema === "webdock_mail" && t.name === "provider_lease")
        sql.push(`REVOKE ALL ON ${q} FROM webdock_auth_runtime,webdock_auth_bootstrap,webdock_registry_runtime; REVOKE DELETE ON ${q} FROM webdock_worker_runtime;`);
      if (t.schema === "webdock_mail" && t.name === "provider_binding")
        sql.push(`REVOKE INSERT,UPDATE,DELETE ON ${q} FROM webdock_worker_runtime; REVOKE DELETE ON ${q} FROM webdock_auth_runtime;`);
      if (t.schema === "webdock_auth" && t.name === "hosting_database_binding_route")
        sql.push(`REVOKE INSERT,UPDATE,DELETE ON ${q} FROM webdock_worker_runtime; REVOKE DELETE ON ${q} FROM webdock_auth_runtime;`);
      if (t.schema === "webdock_auth" && t.name === "repository_lease")
        sql.push(`REVOKE ALL ON ${q} FROM webdock_auth_runtime,webdock_auth_bootstrap,webdock_registry_runtime;`);
      if (t.schema === "webdock_auth" && t.name === "hosting_setup")
        sql.push(
          `GRANT SELECT ON ${q} TO webdock_worker_runtime; GRANT UPDATE(state,reason,resources,steps,generation,lease_until,next_attempt_at,updated_at) ON ${q} TO webdock_worker_runtime;`,
        );
      if (t.schema === "webdock_admin" && t.name === "customers") {
        const profile = [...tenantProfileColumns, "updated_at"];
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_auth_runtime; GRANT SELECT(${["id", "status", "created_at", ...profile].map(ident).join(",")}),UPDATE(${profile.map(ident).join(",")}) ON ${q} TO webdock_auth_runtime;`,
        );
      }
      if (t.schema === "webdock_admin" && t.name === "projects")
        sql.push(
          `REVOKE INSERT,UPDATE,DELETE ON ${q} FROM webdock_auth_runtime;`,
        );
      if (t.schema === "webdock_auth" && t.name === "hosting_database_backend")
        sql.push(`REVOKE DELETE ON ${q} FROM webdock_auth_runtime;`);
      if (t.schema === "webdock_auth" && t.name === "managed_database_launch")
        sql.push(
          `REVOKE UPDATE,DELETE ON ${q} FROM webdock_auth_runtime; REVOKE INSERT,DELETE ON ${q} FROM webdock_worker_runtime; REVOKE UPDATE ON ${q} FROM webdock_worker_runtime; GRANT UPDATE(consumed_at) ON ${q} TO webdock_worker_runtime;`,
        );
      if (t.schema === "webdock_auth" && t.name === "managed_database_session")
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_auth_runtime; REVOKE UPDATE ON ${q} FROM webdock_worker_runtime;`,
        );
      if (t.schema === "webdock_auth" && t.name === "clerk_identity_mapping")
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_worker_runtime; GRANT SELECT(issuer,external_subject,local_subject,revoked_at) ON ${q} TO webdock_worker_runtime;`,
        );
      if (t.schema === "webdock_auth" && t.name === "clerk_session_mapping")
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_worker_runtime; GRANT SELECT(issuer,external_session,external_subject,local_session_id,expires_at) ON ${q} TO webdock_worker_runtime;`,
        );
      // The app's identity joins need no password/token columns or arbitrary identity mutations.
      if (t.schema === "webdock_auth" && t.name === "session") {
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_auth_runtime,webdock_worker_runtime; GRANT SELECT(id,"userId","expiresAt"),UPDATE(id) ON ${q} TO webdock_auth_runtime,webdock_worker_runtime;`,
        );
      }
      if (t.schema === "webdock_auth" && t.name === "mail_api_key") {
        oldWrite.set("webdock_auth_bootstrap", `${col("revoked_at")} IS NULL`);
        sql.push(
          `GRANT SELECT(last_used_at),UPDATE(last_used_at) ON ${q} TO webdock_auth_bootstrap; CREATE POLICY bootstrap_key_seen ON ${q} FOR UPDATE TO webdock_auth_bootstrap USING(revoked_at IS NULL) WITH CHECK(revoked_at IS NULL);`,
        );
      }
      if (t.schema === "webdock_auth" && t.name === "oauthClient") {
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_auth_runtime,webdock_worker_runtime; GRANT SELECT("clientId",name,disabled,scopes,metadata,"redirectUris","createdAt"),UPDATE(disabled),DELETE ON ${q} TO webdock_auth_runtime;`,
        );
      }
      if (t.schema === "webdock_auth" && t.name === "studio_tenant_preview") {
        sql.push(
          `REVOKE UPDATE ON ${q} FROM webdock_auth_runtime; GRANT UPDATE(expired_at) ON ${q} TO webdock_auth_runtime;`,
        );
      }
      if (t.schema === "webdock_auth" && t.name === "user") {
        sql.push(
          `REVOKE ALL ON ${q} FROM webdock_auth_runtime,webdock_worker_runtime; GRANT SELECT(id,name,email,role,banned,"emailVerified","mustChangePassword","twoFactorEnabled","createdAt"),UPDATE(id) ON ${q} TO webdock_auth_runtime,webdock_worker_runtime; GRANT UPDATE(name,email,role,banned,"emailVerified","mustChangePassword","twoFactorEnabled") ON ${q} TO webdock_auth_runtime;`,
        );
      }
      // UPDATE USING(read) is needed for authorization row locks. Actual updates
      // must ALSO be writable before mutation, or a reader of A/admin of B could
      // reparent A's row to B and satisfy only the new-row WITH CHECK.
      const guard = ident(
        "guard_update_" +
          createHash("sha256").update(key).digest("hex").slice(0, 20),
      );
      const checks = [...oldWrite]
        .map(
          ([role, predicate]) =>
            `IF pg_has_role(current_user,${literal(role)},'USAGE') THEN IF coalesce((${predicate.replaceAll(ident(t.name) + ".", "OLD.")}),false) THEN RETURN NEW; END IF; END IF;`,
        )
        .join("\n");
      // Publication pointers belong to deployments; source configuration still requires projects.
      const domainGuard =
        t.schema === "webdock_auth" && t.name === "git_source"
          ? `IF pg_has_role(current_user,'webdock_auth_runtime','USAGE') AND NOT webdock_rls.customer_permission(OLD.customer_id,'org:projects:manage') AND (to_jsonb(NEW)-ARRAY['latest_build_id','environment_identity','desired_release_id']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['latest_build_id','environment_identity','desired_release_id']) THEN RAISE EXCEPTION 'Source configuration permission required' USING ERRCODE='42501'; END IF;`
          : "";
      sql.push(`CREATE OR REPLACE FUNCTION webdock_rls.${guard}() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog,webdock_rls AS $guard$
BEGIN
 IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=current_user AND rolsuper) THEN RETURN NEW; END IF;
 ${domainGuard}
 ${checks}
 RAISE EXCEPTION 'row-level security write authorization required for original row' USING ERRCODE='42501';
END $guard$;
ALTER FUNCTION webdock_rls.${guard}() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.${guard}() FROM PUBLIC;
CREATE TRIGGER aa_webdock_rls_old_write BEFORE UPDATE ON ${q} FOR EACH ROW EXECUTE FUNCTION webdock_rls.${guard}();`);
    } else unknownTables.push(key);
  }
  return { sql: sql.join("\n"), unknownTables, classifiedTables };
}
