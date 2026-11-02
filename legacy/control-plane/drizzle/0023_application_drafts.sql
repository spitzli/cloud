ALTER TABLE webdock_auth.hosting_app ADD COLUMN IF NOT EXISTS lifecycle text NOT NULL DEFAULT 'deployed' CHECK(lifecycle IN ('draft','deployed'));
