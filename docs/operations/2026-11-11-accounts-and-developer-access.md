# Accounts and developer access

The owner selected `https://accounts.spitzli.cloud` as the personal account
surface, with a distinct, expressive Spitzli identity. Console remains the Vite
workspace/project application. Both surfaces use the same separately loaded
account bundle; authentication and permissions remain server-owned.

## Surface and credential boundaries

Set the public Auth origin to the new account host and make the account ingress
forward that exact Host. Serve the same immutable Console build on the Auth
project. Legacy account page navigation and GET/HEAD authorization entry points
forward to Accounts, preserving the raw query. Credential-bearing API requests,
discovery, the retained issuer, signing keys, client registrations and WebAuthn
RP remain compatible. Related-origin WebAuthn metadata must explicitly include
the new account origin. Never share or copy host-only cookies across hosts.
A user may need to sign in once on the new domain; existing credentials are not
reset and already issued tokens are not globally revoked.

The Developer access page links workspace owners/administrators to the correct
Console token page through SSO. MCP uses OAuth with an operator-registered
client and exact callback, rather than a workspace REST API token. The Console
MCP URL forwards to the existing Studio bearer boundary. It does not inherit
Console browser cookies or change the registered resource audience, issuer,
scopes or per-call authorization. Discovery remains authoritative.

## One-time API token display

Creation returns the secret only into the current authorized component state;
it is absent from shared query and mutation caches. Hiding it, changing the
workspace/session/role, or successfully revoking it removes the display. Losing
access aborts the pending read of a creation result, and a late response cannot
restore or cache the secret. It does not cancel a server write already sent.
Creation and revocation do not retry automatically. An uncertain response tells
the user to inspect the token list before sending another operation. Revocation
requires confirmation naming the affected token.

## Qualification and rollout

See the dated verification record for local evidence. Real Core/PG18 tests cover
password/MFA, recovery-code replay denial, passkeys with a virtual authenticator,
SSO, session revocation, OAuth/MCP scopes and revocation, and workspace API-token
management. HTTP fixtures cover UI states, multiple workspaces, phone widths
and both themes. These are distinct checks, not a full production customer E2E
or a physical passkey qualification.

Publish the reviewed source first. Deploy the qualified immutable frontend
build without a remote application build. Retain the previous deployments,
DNS/config snapshots and Core secret hashes. Prepare and verify the Accounts
DNS/TLS binding before changing the canonical origin and account ingress Host;
then publish the matching navigation and Console CSP. Verify exact live assets,
public configuration, discovery, related-origin metadata, MCP challenges and
anonymous security denials. Keep existing native-backend and feature flags.
The larger versioned database/wire/cryptographic rebrand remains a separate
migration; a public account-host change does not complete it.
