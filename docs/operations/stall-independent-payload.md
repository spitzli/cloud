# Stall independent Payload

Stall Eichenbruch already owns `spitzli/stall-eichenbruch`, its Vercel project, the PostgreSQL `stall` schema and its own media prefix. Removing CMS from Webdock does not remove this site or its database.

On 2026-10-17 the mandatory Webdock SSO integration was removed from the standalone source. Payload owns login, logout, session refresh and password recovery. Historical `authSubject` values, account IDs, password hashes and role protection are retained. No data migration or password rotation was performed. A read-only check confirmed the existing operator has a local password hash; a production password login was not attempted.

Production is `dpl_9rbsYnU2vicxCmfatttP6AR6PwEG` with functions in `fra1`; source compiled locally and the prebuilt artifact was promoted. Vercel artifact processing/control plane remains global. The previous deployment `dpl_Fo42o5fta1EkUzeiHKT4DVAAeYeh` remains available for rollback. Production environment `WEBDOCK_SSO_ENFORCE=false` and empty `WEBDOCK_SSO_CLIENT_ID` also prevent the older repository source from forcing SSO on a future deployment.

Homepage and `/admin/login` returned 200 after promotion. The native browser displayed Email, Password and Forgot password on the Stall domain, with no Webdock redirect. `/api/users/me` returns no user anonymously. Payload auth/security tests and an independent review passed.

Source changes are in `/home/newt/.t3/worktrees/stall-independent`, not yet committed or pushed. Before repeating a prebuilt deployment, run `bun scripts/sanitize-prebuilt.mjs` after `vercel build --prod`: the Vercel adapter retained dotenv entries in its function maps despite Next trace exclusions. The sanitizer removes these references; runtime secrets remain supplied by Vercel. Never upload a production dotenv file to satisfy a missing-file build error.
