// Additive, owner-applied schema. Never create tables from a request handler.
export const tenantPreviewSchemaSQL = `CREATE TABLE IF NOT EXISTS webdock_auth.studio_tenant_preview (
 session_id text PRIMARY KEY REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 actor_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE RESTRICT,
 organization_id text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 expires_at timestamptz NOT NULL DEFAULT now() + interval '15 minutes',
 expired_at timestamptz
);`;
