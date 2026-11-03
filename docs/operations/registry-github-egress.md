# Registry GitHub connectivity

The existing Registry application runs in `wd-104618288083894272`, selected by
`webdock.dev/app-id: "104651089088876544"`. Its GitHub access remains restricted
to TCP 443 and reviewed GitHub public addresses. The checked-in policy is
`deploy/backend-gateway/registry-github-egress.json`.

On 2026-11-03, OAuth callbacks exchanged the authorization code successfully,
then failed fetching the GitHub profile with `ConnectionRefused`. A fresh process
in the same Pod resolved a different permitted GitHub API address and succeeded,
which initially hid the problem. A bounded connect-only trace of the serving Bun
process identified `20.250.119.72:443`; it was missing from the October 16 policy.
GitHub's official `https://api.github.com/meta` lists that API address and the
additional web address `20.250.119.67`. Only those two public `/32` destinations
were appended, retaining the exact application selector, HTTPS port and all
existing rules. No identity was unlinked, credential rotated or unrestricted
Internet egress enabled.

After the change, normal exact-commit Registry ingestion succeeded using the
existing identity. The Neon icon, rendered README and three genuine screenshots
were verified publicly, and the existing release signature remained valid.
This proves authenticated catalog fetching after the network repair; a complete
interactive OAuth login was not claimed from that test.

When GitHub changes addresses, compare its official `web` and `api` ranges with
the current policy. Diagnose the serving process's actual destination rather than
assuming a fresh resolver result is identical. Back up the policy, retain its
ownership and use a resource-version check when updating. Never broaden access
to private networks or `0.0.0.0/0` to make an OAuth callback pass. Preserve
existing identity and encryption keys; consumed authorization codes cannot be
replayed.
