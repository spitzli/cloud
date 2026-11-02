import {
  currentDatabaseScope,
  withDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import type { ConsolePrincipal } from "../auth/clerk-identity";
import type { StoredConsoleIdentity } from "../auth/clerk-provider";
import { sourceFail } from "./sources-policy";

/** Auth subjects own RLS rows; Registry profiles separately own GitHub cookies. */
export function sourceOperatorIdentity(
  actor: {
    identity: { subject: string; sessionID: string };
    user: { id: string };
  },
  scope: DatabaseScope | undefined,
  expectedSubject?: string,
) {
  if (
    scope?.kind !== "session" ||
    scope.subject !== actor.identity.subject ||
    scope.sessionID !== actor.identity.sessionID ||
    (expectedSubject !== undefined && scope.subject !== expectedSubject)
  )
    sourceFail(
      "permission_denied",
      "A current operator session is required.",
      403,
    );
  return { id: scope.subject, registryID: actor.user.id, scope };
}

export type SourceSession = Extract<DatabaseScope, { kind: "session" }>;
export function createVerifiedSourceActorRunner(dependencies: {
  revalidate(reference: StoredConsoleIdentity): Promise<ConsolePrincipal>;
  withPrincipal<T>(principal: ConsolePrincipal, work: () => T): T;
  authorize(projectID: string, customerID: string): Promise<void>;
}) {
  return async <T>(
    scope: SourceSession,
    projectID: string,
    customerID: string,
    work: () => Promise<T>,
  ): Promise<T> => {
    if (
      !scope ||
      scope.kind !== "session" ||
      !scope.subject ||
      !scope.sessionID
    )
      sourceFail(
        "permission_denied",
        "A current operator session is required.",
        403,
      );
    const checked = async () => {
      const fresh = currentDatabaseScope();
      if (
        fresh?.kind !== "session" ||
        fresh.subject !== scope.subject ||
        fresh.sessionID !== scope.sessionID
      )
        sourceFail(
          "permission_denied",
          "The verified operator session changed.",
          403,
        );
      await dependencies.authorize(projectID, customerID);
      return work();
    };
    if (scope.clerk) {
      const principal = await dependencies.revalidate({
        issuer: scope.clerk.issuer,
        userID: scope.clerk.userID,
        sessionID: scope.sessionID,
        organizationID: scope.clerk.clerkOrganizationID,
      });
      if (
        principal.userID !== scope.subject ||
        principal.sessionID !== scope.sessionID
      )
        sourceFail(
          "permission_denied",
          "The verified operator session changed.",
          403,
        );
      return dependencies.withPrincipal(principal, checked);
    }
    return withDatabaseScope(scope, checked);
  };
}
