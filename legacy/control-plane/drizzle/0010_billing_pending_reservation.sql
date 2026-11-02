-- Withdrawal cannot invalidate an already issued payment link. Retain its base reservation until resolution.
DROP INDEX webdock_auth.billing_one_website_base;
CREATE UNIQUE INDEX billing_one_website_base ON webdock_auth.billing_contract(environment,customer_id,service_key) WHERE state<>'ended';
