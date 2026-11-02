-- Quote-only drafts have no price; they must never be treated as free products.
ALTER TABLE webdock_auth.billing_product DROP CONSTRAINT billing_product_family_check;
ALTER TABLE webdock_auth.billing_product ADD CONSTRAINT billing_product_family_check CHECK(family IN ('website','media','editors','mail','apps','connect','database','service'));
ALTER TABLE webdock_auth.billing_product_version ALTER COLUMN amount DROP NOT NULL;
ALTER TABLE webdock_auth.billing_product_version DROP CONSTRAINT billing_product_version_interval_check;
ALTER TABLE webdock_auth.billing_product_version ADD CONSTRAINT billing_product_version_interval_check CHECK(interval IN ('month','year','once'));
