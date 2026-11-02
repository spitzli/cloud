-- Test-owned isolated database only. Never a platform migration.
CREATE SCHEMA IF NOT EXISTS webdock_auth;
DO $$ BEGIN IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') THEN GRANT USAGE ON SCHEMA webdock_auth TO webdock_platform_owner; END IF; END $$;
CREATE SEQUENCE IF NOT EXISTS webdock_auth.fixture_id;
CREATE OR REPLACE FUNCTION webdock_auth.next_snowflake() RETURNS text LANGUAGE sql AS $$ SELECT nextval('webdock_auth.fixture_id')::text $$;
CREATE TABLE IF NOT EXISTS webdock_auth."user" (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), name text NOT NULL, email text UNIQUE NOT NULL, "emailVerified" boolean NOT NULL DEFAULT false, role text, banned boolean DEFAULT false, "twoFactorEnabled" boolean DEFAULT false, "mustChangePassword" boolean NOT NULL DEFAULT true, "createdAt" timestamptz NOT NULL DEFAULT now(), "updatedAt" timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS webdock_auth.account (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),"accountId" text NOT NULL,"providerId" text NOT NULL,"userId" text NOT NULL REFERENCES webdock_auth."user"(id),password text,"createdAt" timestamptz DEFAULT now(),"updatedAt" timestamptz NOT NULL);
CREATE TABLE IF NOT EXISTS webdock_auth.session (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),token text UNIQUE NOT NULL,"userId" text NOT NULL REFERENCES webdock_auth."user"(id),"expiresAt" timestamptz NOT NULL,"createdAt" timestamptz DEFAULT now(),"updatedAt" timestamptz NOT NULL);
CREATE TABLE IF NOT EXISTS webdock_auth.studio_tenant_preview (session_id text PRIMARY KEY,actor_id text NOT NULL,customer_id text NOT NULL,organization_id text NOT NULL,expires_at timestamptz NOT NULL,expired_at timestamptz);
CREATE TABLE IF NOT EXISTS webdock_auth."rateLimit" (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),key text UNIQUE NOT NULL,count integer NOT NULL,"lastRequest" bigint NOT NULL);
CREATE TABLE IF NOT EXISTS webdock_auth."twoFactor" (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),secret text NOT NULL,"backupCodes" text NOT NULL,"userId" text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,verified boolean,"failedVerificationCount" integer,"lockedUntil" timestamptz);
CREATE TABLE IF NOT EXISTS webdock_auth.passkey (id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),name text,"publicKey" text NOT NULL,"userId" text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,"credentialID" text NOT NULL,counter integer NOT NULL,"deviceType" text NOT NULL,"backedUp" boolean NOT NULL,transports text,"createdAt" timestamptz,aaguid text);
DELETE FROM webdock_auth.passkey;
DELETE FROM webdock_auth."rateLimit";
DELETE FROM webdock_auth."twoFactor";
DELETE FROM webdock_auth.studio_tenant_preview;
DELETE FROM webdock_auth.session WHERE "userId" IN (SELECT id FROM webdock_auth."user" WHERE email IN ('fixture@example.invalid','operator@example.invalid','recovery@example.invalid','passkey@example.invalid'));
DELETE FROM webdock_auth.account WHERE "userId" IN (SELECT id FROM webdock_auth."user" WHERE email IN ('fixture@example.invalid','operator@example.invalid','recovery@example.invalid','passkey@example.invalid'));
DELETE FROM webdock_auth."user" WHERE email IN ('fixture@example.invalid','operator@example.invalid','recovery@example.invalid','passkey@example.invalid');
