# Intent prefetch in Console and public website

Owner asks to use React/Next.js prefetch for fast native-feeling navigation. Execute inline; keep PC ON and preserve unrelated protected work.

- Console is React19/Vite/TanStack Router1.170, not Next.js. Existing intent preload needs actual lazy-route component preload hooks.
- Use installed TanStack lazyRouteComponent and code-only route loaders restricted to preload requests for existing heavy pages, preserving session/operator gates and Suspense/errors.
- Deliberate hover/focus on project cards may warm one lightweight project summary through the shared Query options. Use existing15s freshness/dedup, abort signal, no retry for speculative reads, session-specific key and current confirmed session guard. Retire delayed intent on unmount/scope change/leave. No plugin discovery, database content, logs, actions or mutations on hover.
- Public Next16.3 website uses internal intent Links for same-origin navigation, hover/focus only. External Console/Auth/contact/hash links stay anchors. Follow installed Next guides; no framework/dependency migration, private RSC/data caching or new provider services.
- RED→GREEN behavior tests cover lazy code before click, cold-route loading with blocked chunks, summary reuse, no unrelated data on hover, cancellation/scope isolation and website link intent. Build/lint/typecheck, focused relevant regressions, one fresh review, signed Blacksmith publication.
- Backend rewrite remains authorized and continues after this steering. No production/schema/credential changes and no general speed/CU claim without measurement.
