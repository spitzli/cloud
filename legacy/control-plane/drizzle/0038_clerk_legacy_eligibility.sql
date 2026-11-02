-- Existing migrated accounts retain their separate legacy login. New projections do not.
ALTER TABLE webdock_auth.clerk_identity ADD COLUMN legacy_eligible boolean NOT NULL DEFAULT true;
