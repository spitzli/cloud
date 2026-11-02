import { retainedMailChecks } from "@webdock/database-runtime";
/** Only these tables receive installation-wide SELECT for the signed preflight scope. */
export const modulePreflightChecks = {
  hosting: [
    [
      "webdock_auth.hosting_app",
      "status <> 'deleted' OR spec->>'volumeBytes' IS NOT NULL",
    ],
    [
      "webdock_auth.hosting_operation",
      "status <> 'succeeded' AND action NOT LIKE 'databases.%'",
    ],
    ["webdock_auth.hosting_setup", "true"],
    [
      "webdock_auth.hosting_reservation",
      "status <> 'released' AND app_id IS NOT NULL",
    ],
    ["webdock_auth.hosting_vercel", "true"],
    ["webdock_auth.git_connection", "true"],
    ["webdock_auth.git_build", "true"],
    ["webdock_auth.git_release", "true"],
    ["webdock_auth.git_check_outbox", "true"],
  ],
  databases: [
    ["webdock_auth.hosting_setup", "configuration #>> '{database,mode}' IN ('new','existing')"],
    ["webdock_auth.hosting_database", "state <> 'purged'"],
    ["webdock_auth.hosting_database_backend", "state <> 'purged'"],
    ["webdock_auth.hosting_database_migration", "state <> 'purged'"],
    ["webdock_auth.hosting_database_order", "true"],
    [
      "webdock_auth.hosting_operation",
      "status <> 'succeeded' AND action LIKE 'databases.%'",
    ],
    ["webdock_auth.database_binding", "true"],
    ["webdock_auth.database_grant", "true"],
  ],
  storage: [["webdock_auth.tenant_storage", "true"]],
} as const;
/** Core must not hide resources retained by an earlier full installation. */
export const coreLegacyRetentionChecks = {
  builtin: [["webdock_auth.plugin_installation", "kind='builtin' AND (state <> 'disabled' OR desired_state <> 'disabled')"]],
  mail: [...retainedMailChecks,
    ["webdock_mail.domain", "true"],
    ["webdock_mail.mailbox", "true"],
    ["webdock_mail.send_transport", "true"],
    ["webdock_auth.mail_connection_secret", "true"],
  ],
  hosting: [...modulePreflightChecks.hosting,
    ["webdock_auth.hosting_cluster", "true"],
    ["webdock_auth.hosting_agent", "NOT revoked"],
    ["webdock_auth.hosting_project", "true"],
    ["webdock_auth.hosting_service_binding", "state <> 'disconnected' OR secret_encrypted IS NOT NULL"],
  ],
  databases: [...modulePreflightChecks.databases,
    ["webdock_auth.database_runtime_owner", "true"],
  ],
  storage: modulePreflightChecks.storage,
} as const;
export const modulePreflightTables = new Set<string>(
  [...Object.values(modulePreflightChecks), ...Object.values(coreLegacyRetentionChecks)].flatMap((checks) =>
    checks.map(([table]) => table),
  ),
);
