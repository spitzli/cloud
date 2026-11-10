import type { DatabaseReader } from "@spitzli/core/identity";
import type { MailProductStore } from "./provider-ports";
import { randomBytes, randomUUID, createHash } from "node:crypto";
import { z } from "zod";
import {
  isMailEvent,
  type EventType,
  type MailActor,
} from "@spitzli/mail-contracts";
import { parseEmail, parseAddress } from "@spitzli/mail-contracts/validation";
import { normalizeMailDomain } from "./tenant-mail";
import { MailError } from "./product-access";
import { redactMailLog } from "./product-request-logs";
import { webhookEndpointProblem } from "./product-webhook-transport";
type Dependencies = {
  encryptMailSecret: import("./provider-ports").MailSecrets["encryptMailSecret"];
  createImportPool: (
    config: import("pg").PoolConfig,
  ) => Pick<import("pg").Pool, "connect" | "end">;
  withMailTransaction: MailProductStore["withMailTransaction"];
  persistMailMessage: ReturnType<
    typeof import("./product-sending").createMailSending
  >["persistMailMessage"];
  canonicalHash: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["canonicalHash"];
  recordMailEvent: ReturnType<
    typeof import("./product-events").createMailEvents
  >["recordMailEvent"];
};
const mappingSchema = z
  .object({
    version: z.literal(1),
    sourceID: z.string().regex(/^[a-zA-Z0-9_-]{1,80}$/),
    providerAccountID: z.string().regex(/^\d{1,64}$/),
    executorStopped: z.literal(true),
    accounts: z
      .array(
        z
          .object({
            legacyAccountID: z.string().min(1).max(200),
            customerID: z.string().regex(/^[1-9]\d{0,18}$/),
            historicalConnectionID: z.string().regex(/^[1-9]\d{0,18}$/),
          })
          .strict(),
      )
      .min(1),
  })
  .strict();
export type RelaykitMapping = z.infer<typeof mappingSchema>;
type Row = Record<string, unknown>;
export function createMailImport(dependencies: Dependencies) {
  const {
    encryptMailSecret,
    createImportPool,
    withMailTransaction,
    persistMailMessage,
    canonicalHash,
    recordMailEvent,
  } = dependencies;
  const string = (v: unknown) => {
    if (typeof v !== "string" || v.length > 4096)
      throw new MailError(422, "invalid_source", "Invalid source field");
    return v;
  };
  const date = (v: unknown) => {
    const at = new Date(v instanceof Date ? v : string(v));
    if (!Number.isFinite(at.getTime()) || at.getTime() > Date.now() + 300000)
      throw new MailError(
        422,
        "invalid_source_time",
        "Invalid source timestamp",
      );
    return at;
  };
  /** Explicit snapshot import. No keys, memberships, provider calls, DNS writes or executor starts. */
  async function importRelaykit(input: {
    sourceReadOnlyURL: string;
    mapping: unknown;
    dryRun: boolean;
    sourceSchema?: string;
  }) {
    const mapping = mappingSchema.parse(input.mapping),
      schema = input.sourceSchema ?? "public";
    if (!/^[a-z][a-z0-9_]{0,62}$/.test(schema))
      throw new MailError(
        422,
        "invalid_source_schema",
        "Invalid source schema",
      );
    if (
      new Set(mapping.accounts.map((a) => a.legacyAccountID)).size !==
      mapping.accounts.length
    )
      throw new MailError(
        422,
        "duplicate_mapping",
        "Duplicate source account mapping",
      );
    // Relaykit's provider credential is instance-wide. Splitting its resources across tenants needs a separate migration design.
    if (
      new Set(
        mapping.accounts.map(
          (a) => a.customerID + ":" + a.historicalConnectionID,
        ),
      ).size !== 1
    )
      throw new MailError(
        409,
        "shared_provider_split_required",
        "Shared provider history cannot be split across tenants",
      );
    const source = createImportPool({
      connectionString: input.sourceReadOnlyURL,
      max: 1,
      connectionTimeoutMillis: 10000,
      options: "-c default_transaction_read_only=on -c statement_timeout=15000",
    });
    const origin = await source.connect().catch(async (error) => {
        await source.end();
        throw error;
      }),
      report = {
        dryRun: input.dryRun,
        source: mapping.sourceID,
        scanned: 0,
        imported: 0,
        existing: 0,
        expired: 0,
        skippedKeys: 0,
        skippedUncertainEvents: 0,
        checksum: "",
      };
    const digest = createHash("sha256");
    const account = (id: unknown) => {
      const found = mapping.accounts.find((a) => a.legacyAccountID === id);
      if (!found)
        throw new MailError(
          422,
          "unmapped_account",
          "Source contains an unmapped resource account",
        );
      return found;
    };
    const table = (name: string) => `"${schema}"."${name}"`;
    const known = new Map<string, string>();
    let cutoff = new Date();
    const lookup = async (db: DatabaseReader, kind: string, id: string) => {
      const key = kind + ":" + id,
        cached = known.get(key);
      if (cached) return cached;
      const row = (
        await db.query(
          "SELECT target_id FROM webdock_auth.mail_import_map WHERE source=$1 AND kind=$2 AND legacy_id=$3",
          [mapping.sourceID, kind, id],
        )
      ).rows[0];
      if (!row)
        throw new MailError(
          422,
          "missing_source_dependency",
          "Source dependency is missing",
        );
      return row.target_id as string;
    };
    const checkDestination = async (db: DatabaseReader) => {
      const flags = (
        await db.query(
          "SELECT settings FROM webdock_auth.platform_settings WHERE id=true FOR SHARE",
        )
      ).rows[0]?.settings;
      if (
        flags?.mailGatewayEnabled === true ||
        flags?.mailWebhooksEnabled === true
      )
        throw new MailError(
          409,
          "gateway_must_be_off",
          "Disable gateway and webhook delivery before import",
        );
      for (const m of mapping.accounts) {
        const c = (
          await db.query(
            "SELECT provider_account_id,state FROM webdock_auth.mail_connection WHERE customer_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE",
            [m.customerID, m.historicalConnectionID],
          )
        ).rows[0];
        if (
          !c ||
          c.provider_account_id !== mapping.providerAccountID ||
          c.state !== "paused"
        )
          throw new MailError(
            409,
            "historical_connection_required",
            "Import requires the original provider connection, explicitly paused",
          );
      }
    };
    try {
      await origin.query("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY");
      await withMailTransaction(async (db) => {
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          "mail-import:" + mapping.sourceID,
        ]);
        await checkDestination(db);
        const checksum = canonicalHash(mapping),
          a = mapping.accounts[0];
        const previous = (
          await db.query(
            "SELECT target_id,checksum FROM webdock_auth.mail_import_map WHERE source=$1 AND kind='snapshot' AND legacy_id='cutoff'",
            [mapping.sourceID],
          )
        ).rows[0];
        if (previous) {
          if (previous.checksum !== checksum)
            throw new MailError(
              409,
              "mapping_changed",
              "Import mapping changed",
            );
          cutoff = date(previous.target_id);
        } else if (!input.dryRun)
          await db.query(
            "INSERT INTO webdock_auth.mail_import_map(source,kind,legacy_id,customer_id,connection_id,target_id,checksum) VALUES($1,'snapshot','cutoff',$2,$3,$4,$5)",
            [
              mapping.sourceID,
              a.customerID,
              a.historicalConnectionID,
              cutoff.toISOString(),
              checksum,
            ],
          );
      });
      // Validate all account ownership before importing the first batch, including organization-attached accounts.
      const owners = await origin.query(
        `SELECT user_id AS account FROM ${table("emails")} UNION SELECT user_id FROM ${table("domains")} UNION SELECT user_id FROM ${table("webhooks")} UNION SELECT user_id FROM ${table("suppressions")} UNION SELECT user_id FROM ${table("request_logs")} UNION SELECT account_id FROM ${table("organization_accounts")} UNION SELECT user_id FROM ${table("api_keys")} UNION SELECT user_id FROM ${table("idempotency_keys")}`,
      );
      for (const row of owners.rows) account(row.account);
      const oversized = await origin.query(
        `SELECT 1 FROM ${table("emails")} WHERE octet_length(payload::text)>4194304 UNION ALL SELECT 1 FROM ${table("request_logs")} WHERE octet_length(coalesce(request_body::text,''))+octet_length(coalesce(response_body::text,''))>4194304 LIMIT 1`,
      );
      if (oversized.rowCount)
        throw new MailError(
          422,
          "source_content_too_large",
          "Source payload exceeds the import limit",
        );
      report.skippedKeys = Number(
        (await origin.query(`SELECT count(*) FROM ${table("api_keys")}`))
          .rows[0].count,
      );
      async function migrate(
        kind: string,
        sql: string,
        handle: (
          db: DatabaseReader,
          row: Row,
          a: RelaykitMapping["accounts"][number],
        ) => Promise<string | null>,
      ) {
        await origin.query(
          `DECLARE migration_cursor NO SCROLL CURSOR FOR ${sql}`,
        );
        try {
          for (;;) {
            const batch = (
              await origin.query("FETCH FORWARD 25 FROM migration_cursor")
            ).rows as Row[];
            if (!batch.length) break;
            await withMailTransaction(async (db) => {
              await db.query(
                "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                ["mail-import:" + mapping.sourceID],
              );
              await checkDestination(db);
              for (const row of batch) {
                const a = account(row.user_id),
                  id = string(row.legacy_id),
                  checksum = canonicalHash(
                    JSON.parse(
                      JSON.stringify({
                        row,
                        account: a,
                        provider: mapping.providerAccountID,
                      }),
                    ),
                  );
                digest.update(kind + ":" + id + ":" + checksum + "\n");
                report.scanned++;
                const previous = (
                  await db.query(
                    "SELECT target_id,checksum,customer_id,connection_id FROM webdock_auth.mail_import_map WHERE source=$1 AND kind=$2 AND legacy_id=$3",
                    [mapping.sourceID, kind, id],
                  )
                ).rows[0];
                if (previous) {
                  if (
                    previous.checksum !== checksum ||
                    previous.customer_id !== a.customerID ||
                    previous.connection_id !== a.historicalConnectionID
                  )
                    throw new MailError(
                      409,
                      "source_changed",
                      "Imported source changed; reconcile before continuing",
                    );
                  known.set(kind + ":" + id, previous.target_id);
                  report.existing++;
                  continue;
                }
                const target = await handle(db, row, a);
                if (target === null) {
                  report.expired++;
                  continue;
                }
                known.set(kind + ":" + id, target);
                report.imported++;
                if (!input.dryRun)
                  await db.query(
                    "INSERT INTO webdock_auth.mail_import_map(source,kind,legacy_id,customer_id,connection_id,target_id,checksum) VALUES($1,$2,$3,$4,$5,$6,$7)",
                    [
                      mapping.sourceID,
                      kind,
                      id,
                      a.customerID,
                      a.historicalConnectionID,
                      target,
                      checksum,
                    ],
                  );
              }
            });
          }
        } finally {
          await origin.query("CLOSE migration_cursor");
        }
      }
      await migrate(
        "domain",
        `SELECT d.turbo_id,d.user_id,d.name,d.created_at,d.turbo_id AS legacy_id,false AS historical FROM ${table("domains")} d
   UNION ALL SELECT NULL,e.user_id,e.domain,min(e.created_at),'history:'||e.user_id||':'||e.domain,true FROM ${table("emails")} e WHERE NOT EXISTS(SELECT 1 FROM ${table("domains")} d WHERE d.user_id=e.user_id AND d.name=e.domain) GROUP BY e.user_id,e.domain ORDER BY legacy_id`,
        async (db, row, a) => {
          const name = normalizeMailDomain(string(row.name));
          date(row.created_at);
          const existing = row.historical
            ? []
            : (
                await db.query(
                  "SELECT id,customer_id FROM webdock_auth.mail_sender_domain WHERE domain=$1 AND deleted_at IS NULL",
                  [name],
                )
              ).rows;
          if (existing.some((d) => d.customer_id !== a.customerID))
            throw new MailError(
              409,
              "domain_mapping_conflict",
              "Source domain is already claimed by another tenant",
            );
          if (input.dryRun)
            return existing[0]?.id ?? "dry-domain:" + row.legacy_id;
          const id =
            existing[0]?.id ??
            (
              await db.query(
                "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,created_at,deleted_at) VALUES($1,$2,$3,$4,$5) RETURNING id",
                [
                  a.customerID,
                  name,
                  randomBytes(24).toString("hex"),
                  date(row.created_at),
                  row.historical ? cutoff : null,
                ],
              )
            ).rows[0].id;
          await db.query(
            "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id,provider_id) VALUES($1,$2,$3,$4) ON CONFLICT(connection_id,domain_id) DO NOTHING",
            [
              a.customerID,
              a.historicalConnectionID,
              id,
              row.turbo_id === null ? null : string(row.turbo_id),
            ],
          );
          return id;
        },
      );
      await migrate(
        "email",
        `SELECT e.*,e.id::text AS legacy_id,coalesce(d.turbo_id,'history:'||e.user_id||':'||e.domain) AS source_domain_id,
   (SELECT max(recent.created_at) FROM ${table("email_events")} recent WHERE recent.email_id=e.id) AS latest_source_event,
   (SELECT min(sent.created_at) FROM ${table("email_events")} sent WHERE sent.email_id=e.id AND sent.type IN ('sent','failed','canceled','suppressed') AND (sent.type<>'suppressed' OR sent.recipient IS NULL)) AS terminal_anchor,
   (SELECT array_agg(DISTINCT suppressed.recipient) FROM ${table("email_events")} suppressed WHERE suppressed.email_id=e.id AND suppressed.type='suppressed' AND suppressed.recipient IS NOT NULL) AS suppressed_recipients,
   EXISTS(SELECT 1 FROM ${table("webhook_deliveries")} delivery WHERE delivery.status='pending' AND coalesce(delivery.payload->'data'->>'email_id',delivery.payload->'data'->>'id')=e.id::text) AS pending_delivery FROM ${table("emails")} e LEFT JOIN ${table("domains")} d ON d.user_id=e.user_id AND d.name=e.domain ORDER BY e.id`,
        async (db, row, a) => {
          const created = date(row.created_at),
            terminal = date(row.terminal_anchor ?? row.created_at),
            state = string(row.last_event);
          if (
            ![
              "scheduled",
              "queued",
              "sent",
              "delivery_delayed",
              "delivered",
              "opened",
              "clicked",
              "bounced",
              "complained",
              "failed",
              "canceled",
              "suppressed",
            ].includes(state)
          )
            throw new MailError(
              422,
              "unknown_source_status",
              "Unknown source status",
            );
          const active = ["scheduled", "queued"].includes(state);
          if (
            !active &&
            !row.pending_delivery &&
            (!row.latest_source_event ||
              date(row.latest_source_event).getTime() <
                cutoff.getTime() - 90 * 86400000) &&
            terminal.getTime() < cutoff.getTime() - 90 * 86400000
          )
            return null;
          const raw = z.record(z.string(), z.unknown()).parse(row.payload);
          const parsed = parseEmail(
            {
              ...raw,
              ...(state !== "scheduled" ? { attachments: undefined } : {}),
              from: row.from,
              to: row.to,
              subject: row.subject,
              ...(state === "scheduled"
                ? {
                    scheduled_at: new Date(
                      string(
                        row.scheduled_at instanceof Date
                          ? row.scheduled_at.toISOString()
                          : row.scheduled_at,
                      ),
                    ).toISOString(),
                  }
                : { scheduled_at: undefined }),
            } as Parameters<typeof parseEmail>[0],
            new Date(),
            { ignoreScheduleTime: true },
          );
          if (parsed.simulated)
            throw new MailError(
              422,
              "source_simulator_unsupported",
              "Source simulator data requires a separate fixture import",
            );
          const domainID = await lookup(
            db,
            "domain",
            string(row.source_domain_id),
          );
          if (
            parsed.fromEmail.split("@")[1] !==
            normalizeMailDomain(string(row.domain))
          )
            throw new MailError(
              422,
              "source_domain_mismatch",
              "Source sender does not match its domain",
            );
          const mid = row.mid === null ? null : string(row.mid);
          if (mid !== null && !/^\d{1,64}$/.test(mid))
            throw new MailError(
              422,
              "invalid_source_mid",
              "Invalid source provider ID",
            );
          const suppressed = new Set(
            (row.suppressed_recipients ?? ([] as unknown[])) as unknown[],
          );
          const suppressedEmails = [...suppressed].map(parseAddress);
          if (
            suppressedEmails.some(
              (email) => !parsed.recipients.some((r) => r.email === email),
            )
          )
            throw new MailError(
              422,
              "invalid_source_recipient",
              "Suppression event does not belong to source envelope",
            );
          if (input.dryRun) return "dry-email:" + row.legacy_id;
          const actor: MailActor = {
            customerID: a.customerID,
            connectionID: a.historicalConnectionID,
            principal: { kind: "job", jobID: "1" },
          };
          const id = await persistMailMessage(db, actor, parsed, domainID),
            status =
              state === "queued"
                ? "outcome_unknown"
                : state === "scheduled"
                  ? "scheduled"
                  : ["failed", "canceled", "suppressed"].includes(state)
                    ? state
                    : "accepted";
          await db.query(
            `UPDATE webdock_auth.mail_message SET dispatch_status=$2,provider_mid=$3,created_at=$4,updated_at=$5,terminal_at=$6,accepted_at=$7,handoff_started_at=$8,principal=$9,error_code=$10 WHERE id=$1`,
            [
              id,
              status,
              mid,
              created,
              terminal,
              active ? null : terminal,
              status === "accepted" ? terminal : null,
              status === "accepted" || status === "outcome_unknown"
                ? terminal
                : null,
              JSON.stringify({
                source: "relaykit",
                sourceID: mapping.sourceID,
                legacyID: row.legacy_id,
              }),
              state === "queued" ? "imported_uncertain_handoff" : null,
            ],
          );
          const expiry = new Date(
            terminal.getTime() + (state === "queued" ? 7 : 30) * 86400000,
          );
          if (state !== "scheduled")
            await db.query(
              "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL,expires_at=$2 WHERE message_id=$1",
              [id, expiry],
            );
          if (state !== "scheduled" && expiry < cutoff)
            await db.query(
              "DELETE FROM webdock_auth.mail_message_content WHERE message_id=$1",
              [id],
            );
          await db.query(
            "UPDATE webdock_auth.mail_recipient SET suppression_reason='legacy_suppressed',delivery_status='suppressed' WHERE message_id=$1 AND email=ANY($2)",
            [id, suppressedEmails],
          );
          if (status === "accepted" || status === "outcome_unknown")
            await db.query(
              "UPDATE webdock_auth.mail_recipient SET included_in_handoff=true WHERE message_id=$1 AND suppression_reason IS NULL",
              [id],
            );
          if (
            (status === "accepted" || status === "outcome_unknown") &&
            parsed.recipients.length > suppressedEmails.length
          ) {
            const period = created.toISOString().slice(0, 7) + "-01",
              count = parsed.recipients.length - suppressedEmails.length,
              unknown = status === "outcome_unknown";
            await db.query(
              "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
              ["mail-budget:" + a.customerID],
            );
            await db.query(
              `INSERT INTO webdock_auth.mail_usage_period(customer_id,connection_id,period,reserved,consumed) VALUES($1,$2,$3,$4,$5) ON CONFLICT(customer_id,connection_id,period) DO UPDATE SET reserved=mail_usage_period.reserved+$4,consumed=mail_usage_period.consumed+$5`,
              [
                a.customerID,
                a.historicalConnectionID,
                period,
                unknown ? count : 0,
                unknown ? 0 : count,
              ],
            );
            await db.query(
              "INSERT INTO webdock_auth.mail_usage_reservation(customer_id,connection_id,message_id,period,recipients,status) VALUES($1,$2,$3,$4,$5,$6)",
              [
                a.customerID,
                a.historicalConnectionID,
                id,
                period,
                count,
                unknown ? "reserved" : "consumed",
              ],
            );
          }
          if (status === "outcome_unknown") {
            await db.query(
              `INSERT INTO webdock_auth.mail_idempotency(customer_id,connection_id,key,method,path,body_hash,status,message_id) VALUES($1,$2,$3,'POST','/legacy-import',$4,'unknown',$5)`,
              [
                a.customerID,
                a.historicalConnectionID,
                "import:" + mapping.sourceID + ":" + row.legacy_id,
                canonicalHash(raw),
                id,
              ],
            );
          }
          return id;
        },
      );
      await migrate(
        "event",
        `SELECT e.*,e.id::text AS legacy_id,m.user_id FROM ${table("email_events")} e JOIN ${table("emails")} m ON m.id=e.email_id ORDER BY e.created_at,e.id`,
        async (db, row, a) => {
          const at = date(row.created_at);
          if (at.getTime() < cutoff.getTime() - 90 * 86400000) return null;
          if (row.type === "queued") {
            report.skippedUncertainEvents++;
            return null;
          }
          const type = "email." + string(row.type);
          if (!isMailEvent(type))
            throw new MailError(
              422,
              "unknown_source_event",
              "Unsupported source event",
            );
          const id = await lookup(db, "email", string(row.email_id)),
            recipient =
              row.recipient === null ? undefined : parseAddress(row.recipient);
          if (input.dryRun) return "dry-event:" + row.legacy_id;
          await recordMailEvent(
            {
              customerID: a.customerID,
              connectionID: a.historicalConnectionID,
              resourceID: id,
              type,
              recipient,
              occurredAt: at.toISOString(),
              source: "reconciliation",
              dedupeKey: "import:" + mapping.sourceID + ":" + row.legacy_id,
            },
            db,
            { outbox: false },
          );
          const event = (
            await db.query(
              "UPDATE webdock_auth.mail_event SET created_at=$3 WHERE connection_id=$1 AND dedupe_key=$2 RETURNING id",
              [
                a.historicalConnectionID,
                "import:" + mapping.sourceID + ":" + row.legacy_id,
                at,
              ],
            )
          ).rows[0];
          return event.id;
        },
      );
      await migrate(
        "suppression",
        `SELECT s.*,user_id||':'||email AS legacy_id FROM ${table("suppressions")} s ORDER BY user_id,email`,
        async (db, row, a) => {
          const email = parseAddress(row.email),
            reason = z
              .enum(["manual", "bounced", "complained"])
              .parse(row.reason),
            at = date(row.created_at);
          if (!input.dryRun)
            await db.query(
              `INSERT INTO webdock_auth.mail_suppression(customer_id,email,reason,source,created_at) VALUES($1,$2,$3,'relaykit',$4) ON CONFLICT(customer_id,email) DO UPDATE SET reason=excluded.reason WHERE mail_suppression.reason NOT IN ('complained','unsubscribed')`,
              [a.customerID, email, reason, at],
            );
          return email;
        },
      );
      await migrate(
        "webhook",
        `SELECT w.*,id::text AS legacy_id FROM ${table("webhooks")} w ORDER BY id`,
        async (db, row, a) => {
          const url = string(row.url),
            secret = string(row.secret),
            events = z.array(z.string()).parse(row.events),
            at = date(row.created_at);
          if (
            webhookEndpointProblem(url) ||
            !/^whsec_[A-Za-z0-9+/]+={0,2}$/.test(secret) ||
            Buffer.from(secret.slice(6), "base64").length < 24 ||
            !events.length ||
            !events.every(isMailEvent)
          )
            throw new MailError(
              422,
              "invalid_source_webhook",
              "Invalid source webhook",
            );
          const id = input.dryRun
            ? "dry-webhook:" + row.legacy_id
            : (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
                .rows[0].id;
          const sealed = encryptMailSecret(
            { secret },
            `mail-webhook:${a.customerID}:${a.historicalConnectionID}:${id}:1`,
          );
          if (!input.dryRun)
            await db.query(
              "INSERT INTO webdock_auth.mail_webhook(id,customer_id,connection_id,url,events,encrypted_secret,enabled,created_at) VALUES($1,$2,$3,$4,$5,$6,false,$7)",
              [
                id,
                a.customerID,
                a.historicalConnectionID,
                url,
                JSON.stringify(events as EventType[]),
                sealed,
                at,
              ],
            );
          return id;
        },
      );
      await migrate(
        "delivery",
        `SELECT d.*,d.id::text AS legacy_id,w.user_id FROM ${table("webhook_deliveries")} d JOIN ${table("webhooks")} w ON w.id=d.webhook_id ORDER BY d.id`,
        async (db, row, a) => {
          const at = date(row.created_at),
            status = z
              .enum(["pending", "succeeded", "failed"])
              .parse(row.status),
            attempts = z.number().int().min(0).max(100).parse(row.attempts);
          if (
            status !== "pending" &&
            at.getTime() < cutoff.getTime() - 30 * 86400000
          )
            return null;
          if (row.lease_until && new Date(String(row.lease_until)) > new Date())
            throw new MailError(
              409,
              "source_lease_active",
              "Wait for the old webhook lease to finish",
            );
          const type = string(row.type);
          if (!isMailEvent(type))
            throw new MailError(
              422,
              "unknown_source_event",
              "Invalid delivery event",
            );
          const payload = z
              .object({ data: z.record(z.string(), z.unknown()) })
              .passthrough()
              .parse(row.payload),
            data = payload.data;
          const candidates = [data.id, data.email_id, data.domain_id].filter(
            (v) => typeof v === "string",
          );
          if (!candidates.length || new Set(candidates).size !== 1)
            throw new MailError(
              422,
              "delivery_resource_unmapped",
              "Delivery resource must be explicitly identifiable",
            );
          let resource: string;
          try {
            resource = await lookup(
              db,
              type.startsWith("email.") ? "email" : "domain",
              string(candidates[0]),
            );
          } catch (error) {
            if (
              type !== "domain.deleted" ||
              !(error instanceof MailError) ||
              error.code !== "missing_source_dependency"
            )
              throw error;
            resource = input.dryRun
              ? "dry-deleted-domain:" + candidates[0]
              : (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
                  .rows[0].id;
            if (!input.dryRun)
              await db.query(
                "INSERT INTO webdock_auth.mail_import_map(source,kind,legacy_id,customer_id,connection_id,target_id,checksum) VALUES($1,'domain',$2,$3,$4,$5,$6)",
                [
                  mapping.sourceID,
                  candidates[0],
                  a.customerID,
                  a.historicalConnectionID,
                  resource,
                  canonicalHash({ deletedDomain: candidates[0], account: a }),
                ],
              );
            known.set("domain:" + candidates[0], resource);
          }
          const endpoint = await lookup(db, "webhook", string(row.webhook_id));
          if (input.dryRun) return "dry-delivery:" + row.legacy_id;
          const event = (
            await db.query(
              `INSERT INTO webdock_auth.mail_event(customer_id,connection_id,resource_id,message_id,type,source,dedupe_key,occurred_at,created_at) VALUES($1,$2,$3,$4,$5,'reconciliation',$6,$7,$7) RETURNING id`,
              [
                a.customerID,
                a.historicalConnectionID,
                resource,
                type.startsWith("email.") ? resource : null,
                type,
                "import:delivery:" + mapping.sourceID + ":" + row.legacy_id,
                at,
              ],
            )
          ).rows[0].id;
          // Attempted-but-pending old deliveries require explicit replay: their previous response may have been lost.
          const targetStatus =
            status === "pending" && attempts > 0 ? "failed" : status;
          const body = JSON.stringify({
            version: 1,
            id: event,
            type,
            created_at: at.toISOString(),
            data: { id: resource, simulated: false },
          });
          const id = (
            await db.query(
              `INSERT INTO webdock_auth.mail_webhook_delivery(customer_id,connection_id,webhook_id,event_id,body,status,attempts,next_at,created_at,finished_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id`,
              [
                a.customerID,
                a.historicalConnectionID,
                endpoint,
                event,
                body,
                targetStatus,
                Math.min(7, attempts),
                row.next_attempt_at ?? at,
                at,
                targetStatus === "pending" ? null : at,
              ],
            )
          ).rows[0].id;
          if (attempts)
            await db.query(
              `INSERT INTO webdock_auth.mail_webhook_attempt(customer_id,connection_id,delivery_id,http_status,response,error_code,duration_ms,created_at) VALUES($1,$2,$3,$4,$5,$6,0,$7)`,
              [
                a.customerID,
                a.historicalConnectionID,
                id,
                typeof row.response_status === "number"
                  ? row.response_status
                  : null,
                JSON.stringify(redactMailLog(row.response)),
                status === "pending" ? "imported_uncertain_delivery" : null,
                at,
              ],
            );
          return id;
        },
      );
      await migrate(
        "log",
        `SELECT l.*,id::text AS legacy_id FROM ${table("request_logs")} l ORDER BY id`,
        async (db, row, a) => {
          const at = date(row.created_at);
          if (at.getTime() < cutoff.getTime() - 30 * 86400000) return null;
          const status = z.number().int().min(100).max(599).parse(row.status),
            duration = z.number().int().min(0).parse(row.duration_ms);
          if (input.dryRun) return "dry-log:" + row.legacy_id;
          const id = (
            await db.query(
              `INSERT INTO webdock_auth.mail_request_log(customer_id,connection_id,request_id,method,route,status,duration_ms,request_body,response_body,created_at) VALUES($1,$2,$3,$4,'/legacy-import',$5,$6,$7,$8,$9) RETURNING id`,
              [
                a.customerID,
                a.historicalConnectionID,
                randomUUID(),
                z
                  .enum([
                    "GET",
                    "POST",
                    "PUT",
                    "PATCH",
                    "DELETE",
                    "OPTIONS",
                    "HEAD",
                  ])
                  .parse(row.method),
                status,
                duration,
                JSON.stringify(redactMailLog(row.request_body)),
                JSON.stringify(redactMailLog(row.response_body)),
                at,
              ],
            )
          ).rows[0].id;
          return id;
        },
      );
      await origin.query("COMMIT");
      report.checksum = digest.digest("hex");
      return report;
    } catch (error) {
      await origin.query("ROLLBACK").catch(() => {});
      throw error;
    } finally {
      origin.release();
      await source.end();
    }
  }
  return { importRelaykit };
}
