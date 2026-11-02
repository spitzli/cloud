-- The release notice now describes current Neon processing and contact-only intake.
-- Preserve every existing native/imported notice version; only future rows change.
ALTER TABLE webdock_auth.waitlist_entry
    ALTER COLUMN privacy_version SET DEFAULT '2026-11-01';
