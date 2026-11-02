import { consoleAuthConfiguration } from "./console-configuration";
import { HostingError } from "@webdock/hosting-contracts";
import { workerDatabase } from "../../db/runtime";
import type { ConsoleIdentityDependencies } from "../auth/clerk-identity";
import { isClerkOrganizationRole, normalizeOrganizationPermissions } from "../auth/organization-permissions";
import {
  clerkProvider,
  liveMemberships,
  revalidateStoredConsoleIdentity,
} from "../auth/clerk-provider";
import { withStoredConsolePrincipal } from "../auth/principal";
import { withDatabaseScope } from "@webdock/database-runtime";
import { executeSetupStep } from "../hosting/setup-execution";
import type { SetupRow } from "@webdock/module-hosting/server/project-setup";
export async function executeAuthorizedSetup(row: SetupRow) {
  const reference = row.actor?.consoleIdentity;
  const principal = reference
    ? await withDatabaseScope(
        {
          kind: "system",
          purpose: "hosting-setup-revalidation",
          customerID: row.customer_id,
          setupID: row.id,
          generation: row.generation,
        },
        async () => {
          const result = await workerDatabase.query(
            "SELECT webdock_rls.setup_clerk_revalidation($1,$2,$3,$4) AS mapping",
            [
              reference.issuer,
              reference.userID,
              reference.sessionID,
              reference.organizationID,
            ],
          );
          const mapping = result.rows[0]?.mapping as {
            identity: Awaited<
              ReturnType<ConsoleIdentityDependencies["identity"]>
            >;
            organization: Awaited<
              ReturnType<ConsoleIdentityDependencies["organization"]>
            >;
          } | null;
          if (!mapping)
            throw new HostingError(
              403,
              "Confirm this setup again before continuing.",
            );
          return revalidateStoredConsoleIdentity(reference, {
            identity: async () => mapping.identity,
            organization: async () => mapping.organization,
          });
        },
      )
    : null;
  if (
    principal &&
    (principal.userID !== row.actor?.subject ||
      principal.sessionID !== row.actor?.sessionID)
  )
    throw new HostingError(403, "Confirm this setup again before continuing.");
  const memberships =
    !reference && consoleAuthConfiguration().requireExistingMapping && process.env.DATABASE_RLS_SECRET
      ? await withDatabaseScope(
          {
            kind: "system",
            purpose: "hosting-setup-revalidation",
            customerID: row.customer_id,
            setupID: row.id,
            generation: row.generation,
          },
          async () => {
            const mapping = (
              await workerDatabase.query(
                "SELECT webdock_rls.setup_legacy_revalidation($1) AS mapping",
                [consoleAuthConfiguration().issuer],
              )
            ).rows[0]?.mapping;
            if (!mapping) return undefined;
            if (
              !mapping.enabled ||
              !mapping.userID ||
              mapping.issuer !== consoleAuthConfiguration().issuer
            )
              throw new HostingError(
                403,
                "Setup authorization changed. Sign in and resume the setup.",
              );
            const user = await clerkProvider().users.getUser(mapping.userID);
            if (
              user.id !== mapping.userID ||
              user.banned ||
              user.locked ||
              !user.emailAddresses.some(
                (email) =>
                  email.id === user.primaryEmailAddressId &&
                  email.verification?.status === "verified",
              )
            )
              throw new HostingError(
                403,
                "Setup authorization changed. Sign in and resume the setup.",
              );
            const member = (await liveMemberships(mapping.userID)).find(
              (member) =>
                member.organization.id === mapping.clerkOrganizationID,
            );
            const verifiedAt = Date.now();
            return {
              subject: row.actor!.subject,
              sessionID: row.actor!.sessionID,
              operator: user.privateMetadata?.webdockRole === "operator",
              twoFactorEnabled: user.twoFactorEnabled === true,
              issuer: mapping.issuer as string,
              userID: mapping.userID as string,
              verifiedAt,
              expiresAt: verifiedAt + 60000,
              roles:
                member && isClerkOrganizationRole(member.role)
                  ? [
                      {
                        organizationID: mapping.organizationID as string,
                        customerID: mapping.customerID as string,
                        role: member.role === "org:admin" ? "admin" : "member",
                        permissions: normalizeOrganizationPermissions(
                          member.permissions,
                        ),
                      },
                    ]
                  : [],
            };
          },
        )
      : undefined;
  const execute = () =>
    withDatabaseScope(
      {
        kind: "system",
        purpose: "hosting-setup",
        authProvider: principal || consoleAuthConfiguration().requireExistingMapping ? "clerk" : "better-auth",
        ...(memberships ? { memberships } : {}),
        ...(principal
          ? {
              clerk: {
                issuer: principal.issuer,
                operator: principal.operator,
                userID: principal.clerkUserID,
                subject: principal.userID,
                sessionID: principal.sessionID,
                organizationID: principal.organizationID,
                clerkOrganizationID: principal.clerkOrganizationID,
                customerID: principal.customerID,
                role: principal.role,
                permissions: principal.permissions,
                verifiedAt: principal.verifiedAt,
                expiresAt: principal.expiresAt,
              },
            }
          : {}),
        customerID: row.customer_id,
        setupID: row.id,
        generation: row.generation,
      },
      () => executeSetupStep(row),
    );
  return await (principal
    ? withStoredConsolePrincipal(principal, execute)
    : execute());
}
