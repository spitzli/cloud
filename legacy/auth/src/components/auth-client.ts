"use client";
/* eslint-disable @next/next/no-location-assign-relative-destination -- Authentication transitions reload the page to synchronize session cookies and the provider's signed query. */

import { createAuthClient } from "better-auth/react";
import {
  inferAdditionalFields,
  organizationClient,
  twoFactorClient,
} from "better-auth/client/plugins";
import { oauthProviderClient } from "@better-auth/oauth-provider/client";
import { passkeyClient } from "@better-auth/passkey/client";
import { accountContinuation } from "../lib/account-continuation";

const identityPlugins = [
  passkeyClient(),
  inferAdditionalFields({
    user: {
      role: { type: ["operator", "user"], required: false, input: false },
      mustChangePassword: { type: "boolean", required: false, input: false },
    },
  }),
  twoFactorClient({
    onTwoFactorRedirect() {
      // Preserve the complete signed query. The provider filters and verifies it.
      window.location.assign(`/two-factor${window.location.search}`);
    },
  }),
] as const;

export const authClient = createAuthClient({
  plugins: [...identityPlugins, oauthProviderClient()],
});
// Account changes must return enrollment data rather than resume OAuth when
// Better Auth refreshes the session cookie. Continue explicitly after setup.
export const accountClient = createAuthClient({
  plugins: [...identityPlugins, organizationClient()],
});

export function goToAccount(data: unknown) {
  // Better Auth handles only the redirects returned by the server.
  if (
    data &&
    typeof data === "object" &&
    (("redirect" in data && data.redirect) ||
      ("twoFactorRedirect" in data && data.twoFactorRedirect))
  )
    return;
  const continuation = accountContinuation(window.location.search);
  if (continuation) {
    window.location.assign(continuation);
    return;
  }
  window.location.assign(`/account${window.location.search}`);
}
