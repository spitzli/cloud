-- Offline, additive identity links. No local login credentials or sessions.
CREATE TABLE webdock_auth.clerk_identity (
 issuer text NOT NULL, clerk_user_id text NOT NULL,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 enabled boolean NOT NULL DEFAULT true,
 PRIMARY KEY(issuer,clerk_user_id), UNIQUE(issuer,user_id)
);
CREATE TABLE webdock_auth.clerk_organization (
 issuer text NOT NULL, clerk_organization_id text NOT NULL,
 organization_id text NOT NULL REFERENCES webdock_auth.organization(id) ON DELETE RESTRICT,
 enabled boolean NOT NULL DEFAULT true,
 PRIMARY KEY(issuer,clerk_organization_id), UNIQUE(issuer,organization_id)
);
CREATE TABLE webdock_auth.console_tenant_preview (
 issuer text NOT NULL, session_id text NOT NULL,
 actor_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE RESTRICT,
 organization_id text NOT NULL REFERENCES webdock_auth.organization(id) ON DELETE RESTRICT,
 created_at timestamptz NOT NULL DEFAULT now(),
 expires_at timestamptz NOT NULL DEFAULT now()+interval '15 minutes', expired_at timestamptz,
 PRIMARY KEY(issuer,session_id)
);
REVOKE ALL ON webdock_auth.clerk_identity,webdock_auth.clerk_organization,webdock_auth.console_tenant_preview FROM PUBLIC;
ALTER TABLE webdock_auth.clerk_identity ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.clerk_identity FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.clerk_organization ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.clerk_organization FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.console_tenant_preview ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.console_tenant_preview FORCE ROW LEVEL SECURITY;
