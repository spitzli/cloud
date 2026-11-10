# Intent prefetch

The Console is React19/Vite with TanStack Router1.170.45 and Query5.104.5; the
public website uses Next16.3.8. No framework or dependency migration is involved.

Console navigation already selected `defaultPreload: intent`. Heavy pages now use
TanStack's preloadable lazy route components, with code-only loaders attached to
the existing route wrappers only for preload requests. Normal cold navigation
keeps the existing Suspense loading state while its chunk arrives. Hover/focus warms their JavaScript without mounting
the page or querying provider/database/log/action endpoints. The intent delay is
125milliseconds. Existing session/operator gates, Suspense and route errors stay
in place; the account-only surface retains its separate lazy loading.

Workspace project cards additionally warm one lightweight project summary after
125milliseconds of deliberate hover, or immediately on keyboard focus. Leaving/blurring/unmounting
or changing session retires delayed intent. The normal project hook and prefetch
share query options and15second freshness; in-flight and fresh work deduplicates.
Keys include project ID, full session/role/MFA/preview/customer scope and API
origin. The prefetch requires a current successful noninvalidated session result,
verified active nonpreview authority and a finite future session expiry. Unknown,
setup, stale/retired or preview authority starts no speculative private read.
Requests carry AbortSignal and are canceled by existing private-cache retirement.
Speculative failures stay silent without retries; normal navigation keeps its
normal error/retry UI. Cached UI data is never authorization proof; Core still
enforces all permissions on every API request/action.

The website's same-origin public links use Next Link and enable full route
prefetch only after hover or keyboard focus. Merely placing a link in the viewport
does not warm it; changing its href resets intent. External Console/Auth/contact
and hash links remain anchors. No private/auth RSC payload or cross-service
session/OAuth redirect is prefetched. Public full-page data may still be fetched
when the visitor deliberately chooses a destination; this is not a blanket
background prefetch of every footer route.

## Local verification

```sh
bun run --filter @spitzli/console build
bun run --filter @spitzli/console lint
bun test --isolate apps/console/tests
WEBDOCK_BROWSER_DIST="$PWD/apps/console/dist" bun apps/console/tests/intent-prefetch-browser.mjs
WEBDOCK_BROWSER_DIST="$PWD/apps/console/dist" bun apps/console/tests/prefetch-loading-browser.mjs
bun test --isolate apps/website/tests/waitlist-release.test.tsx apps/website/tests/sso-policy.test.ts apps/website/tests/cms-bridge-policy.test.ts
bun run --filter @spitzli/web test:prefetch
```

The owned Console browser fixture proves summary warming before click, no query
for an unintended project, reuse on focus/navigation, lazy Connections code before
navigation and no unrelated action/provider read on intent. A second fixture holds
the Connections chunk on direct entry and a cold in-app click; the shell and
loading state must remain visible before the chunk arrives. Unit cases cover
unknown/expired/setup/preview/retired/failed session, distinct actor/origin keys and
abort on cache retirement. Existing workspace/project/resource/customer-mode
browser regressions remain part of qualification.

The Next production test builds a temporary two-route app using the exact source
IntentLink and installed framework. It clears all provider/database configuration,
observes real RSC prefetch on hover/focus and client navigation, then terminates
its local server and removes the owned fixture. It does not contact the public
website or production CMS/database. Website typecheck, touched-file lint and
waitlist/SSO/CMS policy tests qualify the actual changed source; this is not a
production CMS content/render acceptance claim.

No schema, credentials, provider settings or deployment are changed here. Large
existing editor/runtime chunks still produce the existing bundle-size warning;
this change warms likely navigation, it does not claim those chunks got smaller
or a measured general latency/Neon compute improvement. The backend native-driver
rewrite remains a separate ongoing workstream.

References: [TanStack preloading](https://tanstack.com/router/latest/docs/guide/preloading),
[lazyRouteComponent](https://tanstack.com/router/latest/docs/api/router/lazyRouteComponentFunction),
[Next prefetching](https://nextjs.org/docs/app/guides/prefetching). Installed Next
`dist/docs` and installed TanStack implementation/types were checked; Query's
current `query()` API is used instead of its deprecated fetch/prefetch helpers.
