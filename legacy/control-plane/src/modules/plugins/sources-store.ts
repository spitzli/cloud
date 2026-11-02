import { randomUUID } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import type { ReadySource } from "./sources-dispatch";
import {
  actionsPrepareSchema,
  actionsChunkSchema,
  actionsBindSchema,
  actionsMetadataSchema,
  actionsSourceInput,
  requireActionsIdentity,
  type ActionsClaims,
} from "./sources-actions";
import { z } from "zod";
import type { DatabaseScope } from "@webdock/database-runtime";
import type {
  PluginImport,
  PluginSourceView,
} from "@webdock/api-client/plugin-sources";
import {
  approveSourceManifest,
  chunkLimit,
  decodeSourceChunk,
  openSourceSecret,
  parseSourceCommand,
  parseSourceManifest,
  PluginSourceError,
  requireSourceLease,
  runtimeBindingSchema,
  sealSourceSecret,
  sourceFail,
  sourceHash,
  sourceLimit,
  sourceRequestHash,
  sourceView,
  type SourceRow,
} from "./sources-policy";
type Client = {
  query(
    sql: string,
    values?: any[],
  ): Promise<{ rows: SourceRow[]; rowCount?: number | null }>;
  release(): void;
};
type Actor = { id: string; scope: Extract<DatabaseScope, { kind: "session" }> };
type Dependencies = {
  db: { connect(): Promise<Client> };
  token(): string;
  configured(): boolean;
  actor?(): Promise<Actor>;
  github?(
    source: Record<string, any>,
    actorID: string,
  ): Promise<{
    archive: Uint8Array;
    commit: string;
    repository: { name: string };
  }>;
  verifyActor?(
    scope: Extract<DatabaseScope, { kind: "session" }>,
    projectID: string,
    customerID: string,
  ): Promise<boolean>;
  probe?(
    row: SourceRow,
    binding: z.infer<typeof runtimeBindingSchema>,
  ): Promise<void>;
};
const fence =
  "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-sources'))";
const pluginFence =
  "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))";
const jobID = z.string().regex(/^[1-9][0-9]{0,19}$/);
const workerIdentity = z.strictObject({ id: jobID, claimToken: z.uuid() });
const completeSchema = workerIdentity.extend({
  result: z.discriminatedUnion("phase", [
    z.strictObject({
      phase: z.literal("build"),
      manifestRaw: z.string().max(262144),
      sourceSHA256: z.string().regex(/^[a-f0-9]{64}$/),
      commit: z
        .string()
        .regex(/^[a-f0-9]{40}$/)
        .optional(),
    }),
    z.strictObject({
      phase: z.literal("deploy"),
      binding: runtimeBindingSchema,
      clusterID: z.string().min(1).max(200),
    }),
    z.strictObject({
      phase: z.literal("failed"),
      code: z.string().min(1).max(80),
    }),
  ]),
});
export function createPluginSourceService(deps: Dependencies) {
  async function tx<T>(run: (c: Client) => Promise<T>): Promise<T> {
    const c = await deps.db.connect();
    try {
      await c.query("BEGIN");
      await c.query(
        "SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','30s',true)",
      );
      await c.query(fence);
      const result = await run(c);
      await c.query("COMMIT");
      return result;
    } catch (error) {
      await c.query("ROLLBACK");
      throw error;
    } finally {
      c.release();
    }
  }
  async function project(c: Client, projectID: string) {
    const row = (
      await c.query(
        "SELECT p.customer_id FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",
        [projectID],
      )
    ).rows[0];
    if (!row)
      sourceFail(
        "project_unavailable",
        "Select an active project and customer.",
        403,
      );
    return String(row.customer_id);
  }
  async function authorize(c: Client) {
    if (
      !(await c.query("SELECT webdock_rls.operator() AS allowed")).rows[0]
        ?.allowed
    )
      sourceFail(
        "permission_denied",
        "A current operator session is required.",
        403,
      );
  }
  async function expire(c: Client) {
    await c.query(
      "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code=CASE WHEN expires_at<=now() THEN 'expired' ELSE 'lease_expired' END,secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',claim_token=NULL,lease_until=NULL,updated_at=now() WHERE status NOT IN ('enabled','failed') AND (expires_at<=now() OR (status IN ('building','deploying') AND lease_until<=now()))",
    );
    await c.query(
      "DELETE FROM webdock_auth.plugin_source_chunk WHERE import_id IN (SELECT id FROM webdock_auth.plugin_source_import WHERE status IN ('failed','enabled') OR expires_at<=now())",
    );
    await c.query(
      "UPDATE webdock_auth.plugin_source_import SET secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',actions_metadata=NULL WHERE (status IN ('enabled','failed') OR expires_at<=now()) AND (secret_cipher IS NOT NULL OR runtime_cipher IS NOT NULL OR actor_scope<>'{}'::jsonb OR actions_metadata IS NOT NULL)",
    );
  }
  async function row(c: Client, id: string, actor?: Actor) {
    const found = (
      await c.query(
        "SELECT * FROM webdock_auth.plugin_source_import WHERE id=$1 FOR UPDATE",
        [id],
      )
    ).rows[0];
    if (!found || (actor && found.actor_id !== actor.id))
      sourceFail("not_found", "Plugin import is unavailable.", 404);
    if (actor && (await project(c, found.project_id)) !== found.customer_id)
      sourceFail(
        "project_changed",
        "The project customer changed. Create a new import.",
        409,
      );
    return found;
  }
  const configured = () => {
    if (!deps.configured())
      sourceFail(
        "installer_unconfigured",
        "Configure the isolated plugin installer and runtime binding file before importing sources.",
        503,
      );
  };
  async function actor(): Promise<Actor> {
    if (!deps.actor)
      sourceFail(
        "permission_denied",
        "A current operator session is required.",
        403,
      );
    return deps.actor();
  }
  async function putChunk(
    c: Client,
    job: SourceRow,
    offset: number,
    bytes: Buffer,
    state: "uploading" | "building" = "uploading",
  ) {
    if (job.status !== state)
      sourceFail(
        "upload_closed",
        "This import no longer accepts upload chunks.",
      );
    if (offset < job.uploaded_bytes) {
      const old = (
        await c.query(
          "SELECT data FROM webdock_auth.plugin_source_chunk WHERE import_id=$1 AND offset_bytes=$2",
          [job.id, offset],
        )
      ).rows[0];
      if (old && Buffer.from(old.data).equals(bytes)) return job;
      sourceFail(
        "chunk_conflict",
        "This upload offset already contains different bytes.",
      );
    }
    if (
      offset !== job.uploaded_bytes ||
      offset + bytes.length > job.expected_bytes ||
      offset + bytes.length > sourceLimit
    )
      sourceFail(
        "upload_offset",
        "Upload chunks in order and within the declared ZIP size.",
      );
    await c.query(
      "INSERT INTO webdock_auth.plugin_source_chunk(import_id,offset_bytes,data) VALUES($1,$2,$3)",
      [job.id, offset, bytes],
    );
    return (
      await c.query(
        "UPDATE webdock_auth.plugin_source_import SET uploaded_bytes=uploaded_bytes+$2,updated_at=now() WHERE id=$1 RETURNING *",
        [job.id, bytes.length],
      )
    ).rows[0];
  }
  async function assertNewPlugin(c: Client, plugin: string, id: string) {
    await c.query(pluginFence);
    if (
      (
        await c.query(
          "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
          [plugin],
        )
      ).rows.length
    )
      sourceFail(
        "already_installed",
        "This plugin is already installed. Source imports do not replace an existing installation.",
      );
    if (
      (
        await c.query(
          "SELECT id FROM webdock_auth.plugin_source_import WHERE plugin_id=$1 AND id<>$2 AND status IN ('approved','deploying','ready')",
          [plugin, id],
        )
      ).rows.length
    )
      sourceFail(
        "plugin_pending",
        "Another approved import already owns this plugin name.",
      );
  }
  async function create(
    input: Extract<ReturnType<typeof parseSourceCommand>, { action: "create" }>,
    who: Actor,
  ): Promise<PluginImport> {
    const result = await tx(async (c) => {
      await authorize(c);
      await expire(c);
      const customerID = await project(c, input.projectID);
      const requestHash = sourceRequestHash(input, deps.token());
      const previous = (
        await c.query(
          "SELECT * FROM webdock_auth.plugin_source_import WHERE actor_id=$1 AND idempotency_key=$2",
          [who.id, input.idempotencyKey],
        )
      ).rows[0];
      if (previous) {
        if (previous.request_sha256 !== requestHash)
          sourceFail(
            "idempotency_conflict",
            "This request key was already used for a different import.",
          );
        return { job: previous, created: false };
      }
      const counts = (
        await c.query(
          "SELECT count(*)::int AS total,count(*) FILTER(WHERE actor_id=$1)::int AS owned FROM webdock_auth.plugin_source_import WHERE status NOT IN ('enabled','failed') AND expires_at>now()",
          [who.id],
        )
      ).rows[0];
      if (Number(counts.total) >= 32 || Number(counts.owned) >= 3)
        sourceFail(
          "import_limit",
          "Finish an existing import before starting another (three per operator, 32 total).",
          429,
        );
      const src = input.source;
      const job = (
        await c.query(
          "INSERT INTO webdock_auth.plugin_source_import(actor_id,actor_scope,project_id,customer_id,source_kind,source_label,source_url,source_ref,expected_bytes,status,idempotency_key,request_sha256) VALUES($1,$2::jsonb,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING *",
          [
            who.id,
            JSON.stringify(who.scope),
            input.projectID,
            customerID,
            src.kind,
            src.kind === "zip"
              ? src.name
              : src.kind === "git"
                ? src.url
                : "GitHub repository",
            src.kind === "git" ? src.url : null,
            src.kind !== "zip" ? src.ref : null,
            src.kind === "zip" ? src.size : 0,
            src.kind === "git" ? "queued" : "uploading",
            input.idempotencyKey,
            requestHash,
          ],
        )
      ).rows[0];
      if (src.kind === "git" && src.credential) {
        job.secret_cipher = sealSourceSecret(
          src.credential,
          deps.token(),
          job.id + ":git",
        );
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET secret_cipher=$2 WHERE id=$1",
          [job.id, job.secret_cipher],
        );
      }
      return { job, created: true };
    });
    if (!result.created || input.source.kind !== "github")
      return sourceView(result.job);
    try {
      if (!deps.github)
        sourceFail(
          "github_unavailable",
          "Connect GitHub and grant read-only source access.",
          503,
        );
      const downloaded = await deps.github(input.source, who.id);
      if (
        !downloaded.archive.byteLength ||
        downloaded.archive.byteLength > sourceLimit ||
        !/^[a-f0-9]{40}$/.test(downloaded.commit)
      )
        sourceFail(
          "github_unavailable",
          "GitHub returned an invalid or oversized archive.",
          400,
        );
      return await tx(async (c) => {
        await authorize(c);
        await expire(c);
        let job = await row(c, result.job.id, who);
        if (job.status !== "uploading")
          sourceFail(
            "upload_closed",
            "The GitHub import expired before source download finished.",
          );
        job = (
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET expected_bytes=$2,source_label=$3,source_commit=$4 WHERE id=$1 RETURNING *",
            [
              job.id,
              downloaded.archive.byteLength,
              downloaded.repository.name,
              downloaded.commit,
            ],
          )
        ).rows[0];
        for (
          let offset = 0;
          offset < downloaded.archive.byteLength;
          offset += chunkLimit
        )
          job = await putChunk(
            c,
            job,
            offset,
            Buffer.from(downloaded.archive.slice(offset, offset + chunkLimit)),
          );
        job = (
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='queued',updated_at=now() WHERE id=$1 RETURNING *",
            [job.id],
          )
        ).rows[0];
        return sourceView(job);
      });
    } catch {
      await tx(async (c) => {
        await authorize(c);
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code='build_failed',secret_cipher=NULL,actor_scope='{}',updated_at=now() WHERE id=$1 AND actor_id=$2 AND status='uploading'",
          [result.job.id, who.id],
        );
        await expire(c);
      });
      sourceFail(
        "github_unavailable",
        "GitHub source is unavailable. Reconnect and approve Contents: read-only; archives must be at most 32 MiB.",
        400,
      );
    }
  }
  async function activate(id: string, who: Actor) {
    const reviewed = await tx(async (c) => {
      await authorize(c);
      await expire(c);
      const job = await row(c, id, who);
      if (job.status === "enabled") return job;
      if (job.status !== "ready")
        sourceFail(
          "runtime_not_ready",
          "Wait for the installed runtime to become ready.",
        );
      return job;
    });
    if (reviewed.status === "enabled") return sourceView(reviewed);
    const binding = runtimeBindingSchema.parse(
      openSourceSecret(reviewed.runtime_cipher, deps.token(), id + ":runtime"),
    );
    if (!deps.probe)
      sourceFail(
        "runtime_unavailable",
        "Plugin runtime verification is unavailable.",
        503,
      );
    await deps.probe(reviewed, binding);
    return tx(async (c) => {
      await authorize(c);
      await expire(c);
      const job = await row(c, id, who);
      if (job.status === "enabled") return sourceView(job);
      if (
        job.status !== "ready" ||
        job.runtime_cipher !== reviewed.runtime_cipher ||
        job.manifest_sha256 !== reviewed.manifest_sha256
      )
        sourceFail(
          "runtime_changed",
          "Runtime or manifest changed during verification.",
        );
      const manifest = approveSourceManifest(
        job,
        job.manifest_sha256,
        job.approved_capabilities,
        job.configuration,
      );
      await assertNewPlugin(c, manifest.name, id);
      await c.query(
        "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,image_digest,runtime,current_generation) VALUES($1,'external','enabled','enabled',$2,$3,'ready',1)",
        [manifest.name, manifest.version, manifest.image],
      );
      await c.query(
        "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,$2,$3,$4,$5,$6,'installer-worker')",
        [
          manifest.name,
          manifest.version,
          job.manifest_raw,
          job.manifest_sha256,
          manifest.image,
          "operator-source:" + id,
        ],
      );
      await c.query(
        "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,$2::jsonb,$3::jsonb)",
        [
          manifest.name,
          JSON.stringify(job.approved_capabilities),
          JSON.stringify(job.configuration),
        ],
      );
      await c.query(
        "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,cluster_id,namespace,service_name,observed_at) VALUES($1,1,$2,$3,'ready',0,$4,$5,$6,$7,now())",
        [
          manifest.name,
          manifest.version,
          manifest.image,
          sourceHash(binding.credential),
          job.cluster_id,
          binding.namespace,
          binding.service,
        ],
      );
      await c.query(
        "INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4::jsonb)",
        [
          manifest.name,
          job.customer_id,
          job.project_id,
          JSON.stringify(job.approved_capabilities),
        ],
      );
      await c.query(
        "DELETE FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
        [id],
      );
      const enabled = (
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET status='enabled',secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',actions_metadata=NULL,claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1 RETURNING *",
          [id],
        )
      ).rows[0];
      return sourceView(enabled);
    });
  }
  async function command(
    raw: unknown,
  ): Promise<PluginImport | PluginSourceView> {
    const input = parseSourceCommand(raw),
      who = await actor();
    // Commit retention independently: a rejected command must not restore expired secrets.
    await tx(async (c) => {
      await authorize(c);
      await expire(c);
    });
    if (input.action !== "list" && input.action !== "discard") configured();
    if (input.action === "create") return create(input, who);
    if (input.action === "activate") return activate(input.id, who);
    return tx(async (c) => {
      await authorize(c);
      await expire(c);
      if (input.action === "list") {
        await project(c, input.projectID);
        const jobs = (
          await c.query(
            "SELECT * FROM webdock_auth.plugin_source_import WHERE project_id=$1 AND actor_id=$2 ORDER BY created_at DESC LIMIT 50",
            [input.projectID, who.id],
          )
        ).rows;
        return { configured: deps.configured(), imports: jobs.map(sourceView) };
      }
      let job = await row(
        c,
        input.id,
        input.action === "discard" ? undefined : who,
      );
      if (input.action === "discard") {
        if (job.actor_id !== who.id)
          sourceFail("not_found", "Plugin import is unavailable.", 404);
        if (job.status === "failed" && job.error_code === "discarded")
          return sourceView(job);
        if (!["uploading", "queued", "review"].includes(job.status))
          sourceFail(
            "discard_unavailable",
            "Only uploads, queued builds and unapproved review results can be discarded.",
          );
        await c.query(
          "DELETE FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
          [job.id],
        );
        job = (
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code='discarded',secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',actions_metadata=NULL,claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1 RETURNING *",
            [job.id],
          )
        ).rows[0];
        return sourceView(job);
      }
      if (input.action === "chunk")
        job = await putChunk(
          c,
          job,
          input.offset,
          decodeSourceChunk(input.data),
        );
      if (input.action === "submit") {
        if (job.status === "queued") return sourceView(job);
        if (
          job.status !== "uploading" ||
          job.source_kind !== "zip" ||
          !job.expected_bytes ||
          job.uploaded_bytes !== job.expected_bytes
        )
          sourceFail(
            "upload_incomplete",
            "Upload the complete ZIP before submitting it.",
          );
        job = (
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='queued',updated_at=now() WHERE id=$1 RETURNING *",
            [job.id],
          )
        ).rows[0];
      }
      if (input.action === "approve") {
        if (
          ["approved", "deploying", "ready", "enabled"].includes(job.status)
        ) {
          approveSourceManifest(
            job,
            input.manifestSha256,
            input.capabilities,
            input.configuration,
          );
          if (!isDeepStrictEqual(job.configuration, input.configuration))
            sourceFail(
              "approval_changed",
              "This import already has a different approved configuration.",
            );
          if (job.status !== "enabled")
            await c.query(
              "UPDATE webdock_auth.plugin_source_import SET actor_scope=$2::jsonb,updated_at=now() WHERE id=$1",
              [job.id, JSON.stringify(who.scope)],
            );
          return sourceView(job);
        }
        if (job.status !== "review")
          sourceFail(
            "not_reviewable",
            "Only a completed build can be approved.",
          );
        const manifest = approveSourceManifest(
          job,
          input.manifestSha256,
          input.capabilities,
          input.configuration,
        );
        await assertNewPlugin(c, manifest.name, job.id);
        job = (
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='approved',configuration=$2::jsonb,approved_capabilities=$3::jsonb,actor_scope=$4::jsonb,updated_at=now() WHERE id=$1 RETURNING *",
            [
              job.id,
              JSON.stringify(input.configuration),
              JSON.stringify(input.capabilities),
              JSON.stringify(who.scope),
            ],
          )
        ).rows[0];
      }
      return sourceView(job);
    });
  }
  async function nextReady(): Promise<ReadySource | undefined> {
    configured();
    return tx(async (c) => {
      await expire(c);
      // Rotate every attempted candidate, including revoked sessions, so it cannot starve others.
      return (
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET updated_at=now() WHERE id=(SELECT id FROM webdock_auth.plugin_source_import WHERE status='ready' AND expires_at>now() ORDER BY updated_at,id LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id,actor_scope,project_id,customer_id",
        )
      ).rows[0] as ReadySource | undefined;
    });
  }
  async function worker(action: string, input: unknown): Promise<unknown> {
    configured();
    await tx(expire);
    if (action === "claim") {
      z.strictObject({}).parse(input);
      return tx(async (c) => {
        await expire(c);
        const job = (
          await c.query(
            "SELECT * FROM webdock_auth.plugin_source_import WHERE status IN ('queued','approved') AND expires_at>now() ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",
          )
        ).rows[0];
        if (!job) return null;
        if (
          !deps.verifyActor ||
          !(await deps.verifyActor(
            job.actor_scope,
            job.project_id,
            job.customer_id,
          ))
        ) {
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code='authority_revoked',secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}' WHERE id=$1",
            [job.id],
          );
          await expire(c);
          return null;
        }
        const phase = job.status === "queued" ? "build" : "deploy",
          claimToken = randomUUID();
        if (phase === "deploy") await assertNewPlugin(c, job.plugin_id, job.id);
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET status=$2,claim_token=$3,lease_until=now()+interval '45 minutes',updated_at=now() WHERE id=$1",
          [job.id, phase === "build" ? "building" : "deploying", claimToken],
        );
        return {
          id: job.id,
          claimToken,
          phase,
          source: {
            kind: job.source_kind,
            label: job.source_label,
            ...(job.source_url ? { url: job.source_url } : {}),
            ...(job.source_ref ? { ref: job.source_ref } : {}),
            ...(job.source_commit ? { commit: job.source_commit } : {}),
            ...(job.secret_cipher
              ? {
                  credential: openSourceSecret(
                    job.secret_cipher,
                    deps.token(),
                    job.id + ":git",
                  ),
                }
              : {}),
          },
          projectID: job.project_id,
          customerID: job.customer_id,
          sourceSize: job.expected_bytes,
          mode: "build",
          ...(phase === "deploy"
            ? {
                manifestRaw: job.manifest_raw,
                configuration: job.configuration,
              }
            : {}),
        };
      });
    }
    const parsed =
      action === "actions-prepare"
        ? actionsPrepareSchema.parse(input)
        : action === "actions-chunk"
          ? actionsChunkSchema.parse(input)
          : action === "actions-bind"
            ? actionsBindSchema.parse(input)
            : action === "complete"
              ? completeSchema.parse(input)
              : action === "source"
                ? workerIdentity
                    .extend({
                      offset: z.number().int().min(0).max(sourceLimit),
                    })
                    .parse(input)
                : workerIdentity.parse(input);
    return tx(async (c) => {
      await expire(c);
      const job = await row(c, parsed.id);
      if (action === "lease") {
        try {
          requireSourceLease(job, parsed.claimToken);
        } catch {
          return { active: false };
        }
        const active =
          !!deps.verifyActor &&
          (await deps.verifyActor(
            job.actor_scope,
            job.project_id,
            job.customer_id,
          ));
        if (!active) {
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code='authority_revoked',secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',claim_token=NULL,lease_until=NULL WHERE id=$1",
            [job.id],
          );
          await expire(c);
        }
        return { active };
      }
      requireSourceLease(job, parsed.claimToken);
      if (
        !deps.verifyActor ||
        !(await deps.verifyActor(
          job.actor_scope,
          job.project_id,
          job.customer_id,
        ))
      )
        sourceFail(
          "authority_revoked",
          "Operator or project access changed. The installer claim is no longer authorized.",
          403,
        );
      if (
        ["actions-prepare", "actions-chunk", "actions-bind"].includes(action)
      ) {
        if (job.status !== "building")
          sourceFail(
            "wrong_phase",
            "Actions preparation requires a live build claim.",
          );
        if (action === "actions-prepare") {
          const {
            id: _id,
            claimToken: _claim,
            ...fields
          } = parsed as z.infer<typeof actionsPrepareSchema>;
          const metadata = actionsMetadataSchema.parse(fields);
          if (job.actions_metadata) {
            const { runID: _run, ...prior } = actionsMetadataSchema.parse(
              job.actions_metadata,
            );
            if (!isDeepStrictEqual(prior, metadata))
              sourceFail(
                "actions_conflict",
                "This build already has a different prepared Actions source.",
              );
            return { accepted: true };
          }
          if (
            job.source_commit &&
            metadata.commit &&
            job.source_commit !== metadata.commit
          )
            sourceFail(
              "actions_conflict",
              "Prepared source commit differs from the selected source.",
            );
          await c.query(
            "DELETE FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
            [job.id],
          );
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET actions_metadata=$2::jsonb,expected_bytes=$3,uploaded_bytes=0,source_sha256=$4,source_commit=COALESCE($5,source_commit),secret_cipher=NULL,updated_at=now() WHERE id=$1",
            [
              job.id,
              JSON.stringify(metadata),
              metadata.size,
              metadata.sourceSHA256,
              metadata.commit ?? null,
            ],
          );
          return { accepted: true };
        }
        const metadata = actionsMetadataSchema.safeParse(job.actions_metadata);
        if (!metadata.success)
          sourceFail(
            "actions_unprepared",
            "Prepare this build's Actions source first.",
          );
        if (action === "actions-chunk") {
          const chunk = parsed as z.infer<typeof actionsChunkSchema>;
          const changed = await putChunk(
            c,
            job,
            chunk.offset,
            decodeSourceChunk(chunk.data),
            "building",
          );
          return { accepted: true, uploadedBytes: changed.uploaded_bytes };
        }
        const { runID } = parsed as z.infer<typeof actionsBindSchema>;
        if (job.uploaded_bytes !== job.expected_bytes || job.expected_bytes < 1)
          sourceFail(
            "upload_incomplete",
            "Complete the Actions source upload before binding a run.",
          );
        if (metadata.data.runID && metadata.data.runID !== runID)
          sourceFail(
            "actions_conflict",
            "This build is already bound to another Actions run.",
          );
        if (!metadata.data.runID)
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET actions_metadata=$2::jsonb,updated_at=now() WHERE id=$1",
            [job.id, JSON.stringify({ ...metadata.data, runID })],
          );
        return { accepted: true };
      }
      if (action === "source") {
        if (job.status !== "building" || job.source_kind === "git")
          sourceFail(
            "source_unavailable",
            "This claim has no uploaded source.",
          );
        const offset = (
          parsed as { id: string; claimToken: string; offset: number }
        ).offset;
        if (offset === job.uploaded_bytes) return { data: "", done: true };
        const chunk = (
          await c.query(
            "SELECT data FROM webdock_auth.plugin_source_chunk WHERE import_id=$1 AND offset_bytes=$2",
            [job.id, offset],
          )
        ).rows[0];
        if (!chunk || Buffer.from(chunk.data).length > chunkLimit)
          sourceFail(
            "invalid_source_offset",
            "Read source at the next chunk offset.",
            400,
          );
        return {
          data: Buffer.from(chunk.data).toString("base64"),
          done: offset + Buffer.from(chunk.data).length === job.uploaded_bytes,
        };
      }
      if (action !== "complete")
        sourceFail("not_found", "Installer route unavailable.", 404);
      const result = (parsed as z.infer<typeof completeSchema>).result;
      if (result.phase === "failed") {
        await c.query(
          "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code=$2,secret_cipher=NULL,runtime_cipher=NULL,actor_scope='{}',claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1",
          [
            job.id,
            job.status === "building" ? "build_failed" : "deploy_failed",
          ],
        );
        await expire(c);
        return { accepted: true };
      }
      if (result.phase === "build") {
        if (job.status !== "building")
          sourceFail("wrong_phase", "This claim is not building a plugin.");
        try {
          const manifest = parseSourceManifest(result.manifestRaw);
          if (
            job.actions_metadata &&
            (!job.actions_metadata.runID ||
              job.source_sha256 !== result.sourceSHA256)
          )
            sourceFail(
              "invalid_manifest",
              "Built source does not match the prepared Actions run.",
              400,
            );
          if (
            job.source_commit &&
            result.commit &&
            job.source_commit !== result.commit
          )
            sourceFail(
              "invalid_manifest",
              "The built commit differs from the selected GitHub commit.",
              400,
            );
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='review',manifest_raw=$2,manifest_sha256=$3,plugin_id=$4,source_sha256=$5,source_commit=COALESCE($6,source_commit),secret_cipher=NULL,claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1",
            [
              job.id,
              result.manifestRaw,
              sourceHash(result.manifestRaw),
              manifest.name,
              result.sourceSHA256,
              result.commit ?? null,
            ],
          );
        } catch (error) {
          if (!(error instanceof PluginSourceError)) throw error;
          await c.query(
            "UPDATE webdock_auth.plugin_source_import SET status='failed',error_code=$2,secret_cipher=NULL,actor_scope='{}',claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1",
            [
              job.id,
              error.code === "unsupported_lifecycle"
                ? error.code
                : "invalid_manifest",
            ],
          );
          await expire(c);
        }
        return { accepted: true };
      }
      if (job.status !== "deploying")
        sourceFail("wrong_phase", "This claim is not deploying a plugin.");
      await c.query(
        "UPDATE webdock_auth.plugin_source_import SET status='ready',runtime_cipher=$2,cluster_id=$3,claim_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1",
        [
          job.id,
          sealSourceSecret(result.binding, deps.token(), job.id + ":runtime"),
          result.clusterID,
        ],
      );
      return { accepted: true };
    });
  }
  async function actionsSource(raw: unknown, claims: ActionsClaims) {
    configured();
    const input = actionsSourceInput.parse(raw);
    await tx(expire);
    return tx(async (c) => {
      const job = await row(c, input.id),
        metadata = requireActionsIdentity(job.actions_metadata, claims, input);
      if (job.status !== "building" || typeof job.claim_token !== "string")
        sourceFail("lease_lost", "Actions source is no longer available.", 409);
      requireSourceLease(job, job.claim_token);
      if (
        !deps.verifyActor ||
        !(await deps.verifyActor(
          job.actor_scope,
          job.project_id,
          job.customer_id,
        ))
      )
        sourceFail(
          "authority_revoked",
          "The source import no longer has current operator authority.",
          403,
        );
      if (
        job.uploaded_bytes !== job.expected_bytes ||
        job.source_sha256 !== metadata.sourceSHA256
      )
        sourceFail(
          "actions_binding_pending",
          "The prepared source is not complete yet.",
          503,
        );
      const fields = {
        sourceSHA256: metadata.sourceSHA256,
        projectID: job.project_id,
        customerID: job.customer_id,
        ...(job.source_commit ? { commit: job.source_commit } : {}),
      };
      if (input.offset === job.uploaded_bytes)
        return { data: "", done: true, ...fields };
      const chunk = (
        await c.query(
          "SELECT data FROM webdock_auth.plugin_source_chunk WHERE import_id=$1 AND offset_bytes=$2",
          [job.id, input.offset],
        )
      ).rows[0];
      if (!chunk || Buffer.from(chunk.data).length > chunkLimit)
        sourceFail(
          "invalid_source_offset",
          "Read source at the next chunk offset.",
          400,
        );
      return {
        data: Buffer.from(chunk.data).toString("base64"),
        done:
          input.offset + Buffer.from(chunk.data).length === job.uploaded_bytes,
        ...fields,
      };
    });
  }
  const safe = <T extends (...args: any[]) => Promise<any>>(fn: T) =>
    (async (...args: Parameters<T>) => {
      try {
        return await fn(...args);
      } catch (error) {
        if (error instanceof PluginSourceError) throw error;
        if (error instanceof z.ZodError)
          sourceFail("invalid_input", "Invalid plugin installer request.", 400);
        sourceFail(
          "source_unavailable",
          "Plugin import is temporarily unavailable. Reload its status before retrying.",
          503,
        );
      }
    }) as T;
  return {
    command: safe(command),
    worker: safe(worker),
    nextReady: safe(nextReady),
    actionsSource: safe(actionsSource),
  };
}
