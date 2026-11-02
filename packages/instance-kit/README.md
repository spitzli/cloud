# Instance policy

Canonical source for the small per-instance Users policy. Webdock apps import this workspace package at build time; there is no runtime dependency on a central CMS. External website repositories retain pinned vendored copies. Update those copies intentionally and run each instance’s access checks.

The visible `operator` role is reserved to the platform owner. Customer admins cannot update/delete the operator, change its credentials, or assign/promote operator roles. Public first-user creation is blocked, including on an empty database. Protected deletion is blocked even through ordinary local APIs; database-level emergency recovery remains with the hosting operator. Field/collection permissions are enforced server-side.

This is not a covert login bypass. Operator ownership of deployment, secrets, schema and recovery is explicit. Ordinary brute-force lockouts remain in force; operators recover through their own email or platform-managed DB/CLI, not a password that customers can change.

Operator password recovery uses Payload’s normal email token flow. A private, single-use WeakMap marker is set only by actual `forgotPassword` / `resetPassword` operations. The reset token and expiry are still validated by Payload, and reset tokens remain single use. Ordinary updates, customer edits and caller-provided context flags cannot enable the recovery exception. No universal password or login bypass exists.
