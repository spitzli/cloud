import { AsyncLocalStorage } from "node:async_hooks";
import type {
  IdentityDirectory,
  SessionAuthenticator,
  SessionReference,
} from "@webdock/core/identity";
type TrustedSession = {
  reference: SessionReference;
  cookie: string;
  kind: "clerk" | "legacy";
};
/** Request-local compatibility authority is only established after protocol authentication. */
export function createBrowserPrincipal(dependencies: {
  provider: "better-auth" | "clerk";
  local: SessionAuthenticator;
  localDirectory: IdentityDirectory;
  clerkDirectory: () => IdentityDirectory;
}) {
  const trusted = new AsyncLocalStorage<TrustedSession>();
  function withTrustedLocalSession<T>(
    headers: Headers,
    reference: SessionReference,
    kind: TrustedSession["kind"],
    run: () => T,
  ): T {
    return trusted.run(
      {
        reference: { ...reference },
        cookie: headers.get("cookie") ?? "",
        kind,
      },
      run,
    );
  }
  function currentTrustedBrowserReference(): SessionReference | null {
    const scope = trusted.getStore();
    return scope?.kind === "clerk" ? { ...scope.reference } : null;
  }
  function trustedLocalSessionKind(
    headers: Headers,
  ): TrustedSession["kind"] | undefined {
    const scope = trusted.getStore();
    return scope?.cookie && headers.get("cookie") === scope.cookie
      ? scope.kind
      : undefined;
  }
  async function resolveSessionReference(
    headers: Headers,
  ): Promise<SessionReference | null> {
    const scope = trusted.getStore();
    if (scope?.cookie && headers.get("cookie") === scope.cookie) {
      const reference = await dependencies.local.authenticate(headers);
      return reference?.subject === scope.reference.subject &&
        reference.sessionID === scope.reference.sessionID
        ? reference
        : null;
    }
    // Clerk enters through its verified Studio bridge, never through a native cookie fallback.
    return dependencies.provider === "better-auth"
      ? dependencies.local.authenticate(headers)
      : null;
  }
  function selected(): IdentityDirectory {
    return dependencies.provider === "better-auth"
      ? dependencies.localDirectory
      : dependencies.clerkDirectory();
  }
  const selectedIdentityDirectory: IdentityDirectory = {
    session: (...args) => selected().session(...args),
    subject: (...args) => selected().subject(...args),
    membership: (...args) => selected().membership(...args),
  };
  return {
    withTrustedLocalSession,
    currentTrustedBrowserReference,
    trustedLocalSessionKind,
    resolveSessionReference,
    selectedIdentityDirectory,
  };
}
