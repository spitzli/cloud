import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { randomBytes, randomUUID } from "node:crypto";
import { Pool } from "pg";
import {
  ScopedPool,
  withDatabaseScope,
  currentDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import { createPluginSourceService } from "../src/modules/plugins/sources-store";
import { sourceHash } from "../src/modules/plugins/sources-policy";
import { createPluginInstallerDispatch } from "../src/modules/plugins/sources-dispatch";
import { createVerifiedSourceActorRunner } from "../src/modules/plugins/sources-identity";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixtureFile ? test : test.skip)(
  "source imports enforce real restricted-role upload, lifecycle, retention and activation fences",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Owned local RLS fixture required");
    const fixture = JSON.parse(await readFile(fixtureFile!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 2 });
    const api = new ScopedPool(
      { connectionString: fixture.roles.webdock_auth_runtime, max: 4 },
      { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
    );
    const worker = new ScopedPool(
      { connectionString: fixture.roles.webdock_worker_runtime, max: 3 },
      { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
    );
    const users = [randomUUID(), randomUUID()],
      sessions = [randomUUID(), randomUUID()];
    const base = BigInt("98" + Date.now()),
      customer = String(base),
      project = String(base + 1n),
      plugin = "source-test-" + randomBytes(5).toString("hex");
    const scopes = users.map((subject, i) => ({
      kind: "session" as const,
      subject,
      sessionID: sessions[i],
    }));
    const run = <T>(fn: () => Promise<T>, i = 0) =>
      withDatabaseScope(scopes[i], fn);
    const work = <T>(fn: () => Promise<T>, purpose = "plugin-controller") =>
      withDatabaseScope({ kind: "system", purpose }, fn);
    let probes = 0;
    const unready = new Set<string>();
    const installedNames = [plugin];
    let databaseFailure = "";
    const monitored = (pool: typeof api) => ({
      connect: async () => {
        const client = await pool.connect();
        return {
          release: () => client.release(),
          query: async (sql: string, values?: any[]) => {
            try {
              return await client.query(sql, values);
            } catch (error) {
              const failure = error as { code?: string; message?: string };
              databaseFailure = `${failure.code}: ${failure.message}`;
              throw error;
            }
          },
        };
      },
    });
    let configured = true;
    const shared = {
      token: () => "test-installer-token-" + "a".repeat(40),
      configured: () => configured,
      actor: async () => {
        const scope = currentDatabaseScope();
        if (scope?.kind !== "session") throw Error();
        return { id: scope.subject, scope };
      },
      probe: async (job: Record<string, any>) => {
        probes++;
        if (unready.has(job.id))
          throw Object.assign(Error("Runtime binding is propagating"), {
            status: 503,
          });
      },
      verifyActor: async (
        scope: Extract<DatabaseScope, { kind: "session" }>,
        projectID: string,
        customerID: string,
      ) =>
        withDatabaseScope(
          scope,
          async () =>
            !!(
              await api.query(
                "SELECT webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_admin.projects WHERE id=$1 AND customer_id=$2 AND status='active') AS allowed",
                [projectID, customerID],
              )
            ).rows[0]?.allowed,
        ),
    };
    const interactive = createPluginSourceService({
        ...shared,
        db: monitored(api),
      }),
      installer = createPluginSourceService({
        ...shared,
        db: monitored(worker),
      });
    const verified = createVerifiedSourceActorRunner({
      revalidate: async () => {
        throw Error("Unexpected Clerk fixture");
      },
      withPrincipal: () => {
        throw Error("Unexpected Clerk fixture");
      },
      authorize: async (projectID, customerID) => {
        const scope = currentDatabaseScope();
        if (
          scope?.kind !== "session" ||
          !(await shared.verifyActor(scope, projectID, customerID))
        )
          throw Error("Operator session expired or revoked");
      },
    });
    const dispatch = createPluginInstallerDispatch({
      nextReady: installer.nextReady,
      withActor: (job, run) =>
        verified(job.actor_scope, job.project_id, job.customer_id, run),
      activate: (id) => interactive.command({ action: "activate", id }),
      worker: installer.worker,
    });
    const command = (input: unknown, i = 0) =>
      run(() => interactive.command(input), i) as Promise<any>;
    const workerCall = (action: string, input: unknown) =>
      work(() => dispatch(action, input)) as Promise<any>;
    const actionsRead = (input: unknown, claims: any) =>
      work(() => installer.actionsSource(input, claims)) as Promise<any>;
    const actionsMetadata = {
      repositoryID: "123",
      repository: "owner/private-builder",
      workflowRef:
        "owner/private-builder/.github/workflows/plugin.yml@refs/heads/main",
      workflowSHA: "a".repeat(40),
      nonce: "b".repeat(32),
    };
    const actionsClaims = {
      repository_id: "123",
      repository: "owner/private-builder",
      repository_visibility: "private" as const,
      workflow_ref: actionsMetadata.workflowRef,
      workflow_sha: actionsMetadata.workflowSHA,
      run_id: "456",
      run_attempt: "1" as const,
      event_name: "workflow_dispatch" as const,
    };
    const zip = (key: string) => ({
      action: "create",
      projectID: project,
      source: { kind: "zip", name: "source.zip", size: 10 },
      mode: "build",
      idempotencyKey: key,
    });
    const manifest = JSON.parse(
      await readFile(
        new URL(
          "../../../tools/plugins/fixtures/project-notes.json",
          import.meta.url,
        ),
        "utf8",
      ),
    );
    Object.assign(manifest, { name: plugin, jobs: [] });
    const raw = JSON.stringify(manifest),
      hash = sourceHash(raw);
    const binding = {
      namespace: "plugin-test",
      service: "fixture",
      addresses: ["127.0.0.1"],
      credential: "runtime-" + "r".repeat(40),
      ca: "test-certificate",
      generation: 1 as const,
      projects: {},
    };
    try {
      const installed = (
        await owner.query(
          "SELECT 1 FROM webdock_api_migrations.journal WHERE id='0046_plugin_sources.sql'",
        )
      ).rows;
      expect(installed).toHaveLength(1);
      for (let i = 0; i < 2; i++) {
        await owner.query(
          'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,\'Source operator\',$2,\'operator\',true,false,true)',
          [users[i], users[i] + "@example.invalid"],
        );
        await owner.query(
          'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'2 hours\',now())',
          [sessions[i], users[i]],
        );
      }
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Source test','active')",
        [customer],
      );
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Source test',$2,'active')",
        [project, customer],
      );
      const job = await command(zip("initial"));
      expect(job.status).toBe("uploading");
      expect((await command(zip("initial"))).id).toBe(job.id);
      await expect(
        command({
          ...zip("initial"),
          source: { kind: "zip", name: "different.zip", size: 10 },
        }),
      ).rejects.toThrow();
      const chunk = {
        action: "chunk",
        id: job.id,
        offset: 0,
        data: Buffer.from("hello").toString("base64"),
      };
      expect((await command(chunk)).uploadedBytes).toBe(5);
      expect((await command(chunk)).uploadedBytes).toBe(5);
      await expect(
        command({ ...chunk, data: Buffer.from("other").toString("base64") }),
      ).rejects.toThrow();
      await expect(command({ action: "submit", id: job.id })).rejects.toThrow();
      expect(
        (
          await run(
            () =>
              api.query(
                "SELECT data FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
                [job.id],
              ),
            1,
          )
        ).rows,
      ).toEqual([]);
      expect(
        (
          await run(
            () =>
              api.query(
                "DELETE FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
                [job.id],
              ),
            1,
          )
        ).rowCount,
      ).toBe(0);
      await expect(command({ ...chunk, offset: 5 }, 1)).rejects.toThrow();
      expect(
        (
          await work(
            () =>
              worker.query("SELECT * FROM webdock_auth.plugin_source_chunk"),
            "mail-cron",
          )
        ).rows,
      ).toEqual([]);
      expect(
        (
          await command({
            ...chunk,
            offset: 5,
            data: Buffer.from("world").toString("base64"),
          })
        ).uploadedBytes,
      ).toBe(10);
      expect((await command({ action: "submit", id: job.id })).status).toBe(
        "queued",
      );
      expect((await command({ action: "submit", id: job.id })).status).toBe(
        "queued",
      );
      const claim = await workerCall("claim", {});
      await expect(
        command({ action: "discard", id: job.id }),
      ).rejects.toThrow();
      expect(claim.id).toBe(job.id);
      expect(claim.sourceSize).toBe(10);
      expect(
        await workerCall("source", {
          id: job.id,
          claimToken: claim.claimToken,
          offset: 0,
        }),
      ).toEqual({ data: chunk.data, done: false });
      expect(
        (
          await workerCall("source", {
            id: job.id,
            claimToken: claim.claimToken,
            offset: 5,
          })
        ).done,
      ).toBe(true);
      expect(
        await workerCall("lease", { id: job.id, claimToken: randomUUID() }),
      ).toEqual({ active: false });
      const preparedBytes = Buffer.from("prepared-zip"),
        prepared = {
          id: job.id,
          claimToken: claim.claimToken,
          size: preparedBytes.length,
          sourceSHA256: "a".repeat(64),
          commit: "f".repeat(40),
          ...actionsMetadata,
        };
      await expect(
        workerCall("actions-prepare", {
          ...prepared,
          claimToken: randomUUID(),
        }),
      ).rejects.toThrow();
      expect(await workerCall("actions-prepare", prepared)).toEqual({
        accepted: true,
      });
      expect(
        (
          await owner.query(
            "SELECT uploaded_bytes FROM webdock_auth.plugin_source_import WHERE id=$1",
            [job.id],
          )
        ).rows[0].uploaded_bytes,
      ).toBe(0);
      expect(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
            [job.id],
          )
        ).rows[0].n,
      ).toBe(0);
      const actionsInput = {
        id: job.id,
        nonce: actionsMetadata.nonce,
        offset: 0,
      };
      await expect(
        actionsRead(actionsInput, actionsClaims),
      ).rejects.toMatchObject({ status: 503, code: "actions_binding_pending" });
      await expect(
        workerCall("actions-bind", {
          id: job.id,
          claimToken: claim.claimToken,
          runID: "456",
        }),
      ).rejects.toThrow();
      const upload = {
        id: job.id,
        claimToken: claim.claimToken,
        offset: 0,
        data: preparedBytes.subarray(0, 6).toString("base64"),
      };
      expect(await workerCall("actions-chunk", upload)).toEqual({
        accepted: true,
        uploadedBytes: 6,
      });
      expect(await workerCall("actions-chunk", upload)).toEqual({
        accepted: true,
        uploadedBytes: 6,
      });
      await expect(
        workerCall("actions-chunk", { ...upload, offset: 8 }),
      ).rejects.toThrow();
      await expect(
        workerCall("actions-chunk", {
          ...upload,
          data: Buffer.from("other!").toString("base64"),
        }),
      ).rejects.toThrow();
      await workerCall("actions-chunk", {
        ...upload,
        offset: 6,
        data: preparedBytes.subarray(6).toString("base64"),
      });
      await workerCall("actions-prepare", prepared);
      expect(
        (
          await owner.query(
            "SELECT uploaded_bytes FROM webdock_auth.plugin_source_import WHERE id=$1",
            [job.id],
          )
        ).rows[0].uploaded_bytes,
      ).toBe(preparedBytes.length);
      await expect(
        workerCall("actions-prepare", { ...prepared, nonce: "e".repeat(32) }),
      ).rejects.toThrow();
      expect(
        await workerCall("actions-bind", {
          id: job.id,
          claimToken: claim.claimToken,
          runID: "456",
        }),
      ).toEqual({ accepted: true });
      expect(
        await workerCall("actions-bind", {
          id: job.id,
          claimToken: claim.claimToken,
          runID: "456",
        }),
      ).toEqual({ accepted: true });
      await expect(
        workerCall("actions-bind", {
          id: job.id,
          claimToken: claim.claimToken,
          runID: "789",
        }),
      ).rejects.toThrow();
      const ciSource = await actionsRead(actionsInput, actionsClaims);
      expect(ciSource).toEqual({
        data: upload.data,
        done: false,
        sourceSHA256: "a".repeat(64),
        projectID: project,
        customerID: customer,
        commit: "f".repeat(40),
      });
      expect(
        (await actionsRead({ ...actionsInput, offset: 6 }, actionsClaims)).done,
      ).toBe(true);
      for (const changed of [
        { repository_id: "999" },
        { workflow_sha: "e".repeat(40) },
        { workflow_ref: actionsMetadata.workflowRef + "x" },
        { run_id: "999" },
        { run_attempt: "2" },
        { repository_visibility: "public" },
      ])
        await expect(
          actionsRead(actionsInput, { ...actionsClaims, ...changed }),
        ).rejects.toMatchObject({ status: 401 });
      await expect(
        actionsRead({ ...actionsInput, nonce: "e".repeat(32) }, actionsClaims),
      ).rejects.toMatchObject({ status: 401 });
      expect(
        (
          await run(
            () =>
              api.query(
                "SELECT data FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
                [job.id],
              ),
            1,
          )
        ).rows,
      ).toEqual([]);
      await expect(
        workerCall("complete", {
          id: job.id,
          claimToken: randomUUID(),
          result: {
            phase: "build",
            manifestRaw: raw,
            sourceSHA256: "a".repeat(64),
          },
        }),
      ).rejects.toThrow();
      const completion = {
        id: job.id,
        claimToken: claim.claimToken,
        result: {
          phase: "build",
          manifestRaw: raw,
          sourceSHA256: "a".repeat(64),
        },
      };
      expect(await workerCall("complete", completion)).toEqual({
        accepted: true,
      });
      await expect(workerCall("complete", completion)).rejects.toThrow();
      const approval = {
        action: "approve",
        id: job.id,
        manifestSha256: hash,
        capabilities: manifest.capabilities,
        configuration: {},
      };
      await expect(
        command({ ...approval, manifestSha256: "0".repeat(64) }),
      ).rejects.toThrow();
      await expect(
        command({ ...approval, capabilities: [] }),
      ).rejects.toThrow();
      expect((await command(approval)).status).toBe("approved");
      await expect(
        command({ action: "discard", id: job.id }),
      ).rejects.toThrow();
      expect((await command(approval)).status).toBe("approved");
      const deploy = await workerCall("claim", {});
      await expect(
        command({ action: "discard", id: job.id }),
      ).rejects.toThrow();
      expect(deploy.phase).toBe("deploy");
      expect(deploy.manifestRaw).toBe(raw);
      expect(deploy.configuration).toEqual({});
      expect(
        await workerCall("complete", {
          id: job.id,
          claimToken: deploy.claimToken,
          result: { phase: "deploy", binding, clusterID: "test-cluster" },
        }),
      ).toEqual({ accepted: true });
      const ready = (await command({ action: "list", projectID: project }))
        .imports[0];
      expect(ready.status).toBe("ready");
      expect(JSON.stringify(ready)).not.toContain("private-builder");
      await expect(actionsRead(actionsInput, actionsClaims)).rejects.toThrow();
      await expect(
        command({ action: "discard", id: job.id }),
      ).rejects.toThrow();
      expect(JSON.stringify(ready)).not.toContain(binding.credential);
      const results = await Promise.all([
        command({ action: "activate", id: job.id }),
        command({ action: "activate", id: job.id }),
        workerCall("claim", {}),
      ]);
      expect(results.slice(0, 2).every((r) => r.status === "enabled")).toBe(
        true,
      );
      expect(results[2]).toBeNull();
      await expect(
        command({ action: "discard", id: job.id }),
      ).rejects.toThrow();
      expect(probes).toBeGreaterThan(0);
      const release = (
        await owner.query(
          "SELECT registry_url,signer_key_id FROM webdock_auth.plugin_release WHERE plugin_id=$1",
          [plugin],
        )
      ).rows[0];
      expect(release).toEqual({
        registry_url: "operator-source:" + job.id,
        signer_key_id: "installer-worker",
      });
      expect(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
            [job.id],
          )
        ).rows[0].n,
      ).toBe(0);
      expect(
        (
          await owner.query(
            "SELECT secret_cipher,runtime_cipher,actor_scope FROM webdock_auth.plugin_source_import WHERE id=$1",
            [job.id],
          )
        ).rows[0],
      ).toEqual({ secret_cipher: null, runtime_cipher: null, actor_scope: {} });
      expect(
        (
          await owner.query(
            "SELECT actions_metadata FROM webdock_auth.plugin_source_import WHERE id=$1",
            [job.id],
          )
        ).rows[0].actions_metadata,
      ).toBeNull();
      const prepareReady = async (
        suffix: string,
        reviewSession?: ReturnType<typeof randomUUID>,
      ) => {
        const name = plugin + suffix;
        installedNames.push(name);
        const imported = await command({
          action: "create",
          projectID: project,
          source: { kind: "git", url: "https://example.com/repo" },
          mode: "build",
          idempotencyKey: suffix,
        });
        const build = await workerCall("claim", {});
        expect(build.id).toBe(imported.id);
        const bytes = JSON.stringify({ ...manifest, name });
        await workerCall("complete", {
          id: imported.id,
          claimToken: build.claimToken,
          result: {
            phase: "build",
            manifestRaw: bytes,
            sourceSHA256: "d".repeat(64),
          },
        });
        const old = scopes[0];
        if (reviewSession) scopes[0] = { ...old, sessionID: reviewSession };
        try {
          await command({
            action: "approve",
            id: imported.id,
            manifestSha256: sourceHash(bytes),
            capabilities: manifest.capabilities,
            configuration: {},
          });
        } finally {
          scopes[0] = old;
        }
        const deployment = await workerCall("claim", {});
        expect(deployment.id).toBe(imported.id);
        expect(deployment.phase).toBe("deploy");
        await workerCall("complete", {
          id: imported.id,
          claimToken: deployment.claimToken,
          result: { phase: "deploy", binding, clusterID: "test-cluster" },
        });
        return imported.id;
      };
      const status = async (id: string) =>
        (
          await owner.query(
            "SELECT status FROM webdock_auth.plugin_source_import WHERE id=$1",
            [id],
          )
        ).rows[0].status;
      const background = await prepareReady("-background");
      expect(await status(background)).toBe("ready");
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(background)).toBe("enabled");
      const revokedReady = await prepareReady("-revoked-ready"),
        beforeRevocation = probes;
      await owner.query(
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        [users[0]],
      );
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(revokedReady)).toBe("ready");
      expect(probes).toBe(beforeRevocation);
      expect(
        (
          await owner.query(
            "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
            [plugin + "-revoked-ready"],
          )
        ).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
        [users[0]],
      );
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(revokedReady)).toBe("enabled");
      const temporary = await prepareReady("-temporary");
      unready.add(temporary);
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET updated_at=now()-interval '1 hour' WHERE id=$1",
        [temporary],
      );
      const before = (
        await owner.query(
          "SELECT updated_at FROM webdock_auth.plugin_source_import WHERE id=$1",
          [temporary],
        )
      ).rows[0].updated_at;
      const nextBuild = await command({
        action: "create",
        projectID: project,
        source: { kind: "git", url: "https://example.com/repo" },
        mode: "build",
        idempotencyKey: "after-unready",
      });
      const nextClaim = await workerCall("claim", {});
      expect(nextClaim.id).toBe(nextBuild.id);
      expect(await status(temporary)).toBe("ready");
      expect(
        (
          await owner.query(
            "SELECT updated_at>$2 AS advanced FROM webdock_auth.plugin_source_import WHERE id=$1",
            [temporary, before],
          )
        ).rows[0].advanced,
      ).toBe(true);
      await workerCall("complete", {
        id: nextBuild.id,
        claimToken: nextClaim.claimToken,
        result: { phase: "failed", code: "fixture-cleanup" },
      });
      const reviewSession = randomUUID();
      await owner.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [reviewSession, users[0]],
      );
      const staleReady = await prepareReady("-stale-session", reviewSession);
      expect(
        (
          await owner.query(
            "SELECT actor_scope->>'sessionID' AS session FROM webdock_auth.plugin_source_import WHERE id=$1",
            [staleReady],
          )
        ).rows[0].session,
      ).toBe(reviewSession);
      await owner.query(
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 minute' WHERE id=$1",
        [reviewSession],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET updated_at=now()-interval '2 hours' WHERE id=$1",
        [staleReady],
      );
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(staleReady)).toBe("ready");
      expect(
        (
          await owner.query(
            "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
            [plugin + "-stale-session"],
          )
        ).rows,
      ).toEqual([]);
      const healthy = await prepareReady("-healthy");
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET updated_at=now()-CASE id WHEN $1 THEN interval '3 hours' WHEN $2 THEN interval '2 hours' ELSE interval '1 hour' END WHERE id=ANY($3)",
        [temporary, staleReady, [temporary, staleReady, healthy]],
      );
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(healthy)).toBe("ready");
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(healthy)).toBe("ready");
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(healthy)).toBe("enabled");
      expect(await status(temporary)).toBe("ready");
      expect(await status(staleReady)).toBe("ready");
      expect(
        (await command({ action: "activate", id: staleReady })).status,
      ).toBe("enabled");
      unready.delete(temporary);
      expect(await workerCall("claim", {})).toBeNull();
      expect(await status(temporary)).toBe("enabled");
      const interrupted = await command(zip("discard-upload"));
      await command({ ...chunk, id: interrupted.id });
      await expect(
        command({ action: "discard", id: interrupted.id }, 1),
      ).rejects.toThrow();
      configured = false;
      const discarded = await command({
        action: "discard",
        id: interrupted.id,
      });
      expect(discarded.status).toBe("failed");
      expect(discarded.error).toBe("Import discarded.");
      expect(await command({ action: "discard", id: interrupted.id })).toEqual(
        discarded,
      );
      configured = true;
      expect(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
            [interrupted.id],
          )
        ).rows[0].n,
      ).toBe(0);
      expect(
        (
          await owner.query(
            "SELECT actor_scope,secret_cipher,runtime_cipher,claim_token,lease_until,error_code FROM webdock_auth.plugin_source_import WHERE id=$1",
            [interrupted.id],
          )
        ).rows[0],
      ).toEqual({
        actor_scope: {},
        secret_cipher: null,
        runtime_cipher: null,
        claim_token: null,
        lease_until: null,
        error_code: "discarded",
      });
      const privateQueued = await command({
        action: "create",
        projectID: project,
        source: {
          kind: "git",
          url: "https://example.com/repo",
          credential: { username: "me", token: "discard-private-token" },
        },
        mode: "build",
        idempotencyKey: "discard-queued",
      });
      expect(
        (
          await owner.query(
            "SELECT secret_cipher FROM webdock_auth.plugin_source_import WHERE id=$1",
            [privateQueued.id],
          )
        ).rows[0].secret_cipher,
      ).not.toBeNull();
      expect(
        (await command({ action: "discard", id: privateQueued.id })).error,
      ).toBe("Import discarded.");
      expect(
        (
          await owner.query(
            "SELECT secret_cipher FROM webdock_auth.plugin_source_import WHERE id=$1",
            [privateQueued.id],
          )
        ).rows[0].secret_cipher,
      ).toBeNull();
      const declined = await command(zip("discard-review"));
      await command({
        ...chunk,
        id: declined.id,
        data: Buffer.from("helloworld").toString("base64"),
      });
      await command({ action: "submit", id: declined.id });
      const declinedClaim = await workerCall("claim", {});
      await workerCall("complete", {
        ...completion,
        id: declined.id,
        claimToken: declinedClaim.claimToken,
      });
      const reviewDiscard = await command({
        action: "discard",
        id: declined.id,
      });
      expect(reviewDiscard.status).toBe("failed");
      expect(reviewDiscard.manifestSha256).toBe(hash);
      expect(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_auth.plugin_source_chunk WHERE import_id=$1",
            [declined.id],
          )
        ).rows[0].n,
      ).toBe(0);
      const racing = await command({
        action: "create",
        projectID: project,
        source: { kind: "git", url: "https://example.com/repo" },
        mode: "build",
        idempotencyKey: "discard-race",
      });
      const race = await Promise.allSettled([
        workerCall("claim", {}),
        command({ action: "discard", id: racing.id }),
      ]);
      const raceRow = (
        await owner.query(
          "SELECT status,error_code,claim_token FROM webdock_auth.plugin_source_import WHERE id=$1",
          [racing.id],
        )
      ).rows[0];
      if (raceRow.status === "building") {
        expect(race[0].status).toBe("fulfilled");
        expect(race[1].status).toBe("rejected");
        expect(raceRow.error_code).toBeNull();
        await workerCall("complete", {
          id: racing.id,
          claimToken: raceRow.claim_token,
          result: { phase: "failed", code: "test-cleanup" },
        });
      } else {
        expect(raceRow.status).toBe("failed");
        expect(raceRow.error_code).toBe("discarded");
        expect(race[0]).toEqual({ status: "fulfilled", value: null });
        expect(race[1].status).toBe("fulfilled");
      }
      const limited = [];
      for (let i = 0; i < 3; i++)
        limited.push(await command(zip("limit-" + i)));
      await expect(command(zip("over-limit"))).rejects.toThrow(
        "Finish an existing import",
      );
      await command({ action: "discard", id: limited[0].id });
      expect((await command(zip("slot-recovered"))).status).toBe("uploading");
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET expires_at=now()-interval '1 second' WHERE actor_id=$1 AND status<>'enabled'",
        [users[0]],
      );
      await command({ action: "list", projectID: project });
      const expired = await command({
        action: "create",
        projectID: project,
        source: {
          kind: "git",
          url: "https://example.com/repo",
          credential: { username: "me", token: "private-git-token" },
        },
        mode: "build",
        idempotencyKey: "expired-lease",
      });
      const expiredClaim = await workerCall("claim", {});
      expect(expiredClaim.id).toBe(expired.id);
      expect(expiredClaim.source.credential.token).toBe("private-git-token");
      await workerCall("actions-prepare", {
        ...prepared,
        id: expired.id,
        claimToken: expiredClaim.claimToken,
      });
      expect(
        (
          await owner.query(
            "SELECT secret_cipher FROM webdock_auth.plugin_source_import WHERE id=$1",
            [expired.id],
          )
        ).rows[0].secret_cipher,
      ).toBeNull();
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET lease_until=now()-interval '1 second' WHERE id=$1",
        [expired.id],
      );
      await expect(
        workerCall("complete", {
          ...completion,
          id: expired.id,
          claimToken: expiredClaim.claimToken,
        }),
      ).rejects.toThrow();
      expect(await workerCall("claim", {})).toBeNull();
      const retained = (
        await owner.query(
          "SELECT status,error_code,secret_cipher FROM webdock_auth.plugin_source_import WHERE id=$1",
          [expired.id],
        )
      ).rows[0];
      expect(retained).toEqual({
        status: "failed",
        error_code: "lease_expired",
        secret_cipher: null,
      });
      expect(
        (
          await owner.query(
            "SELECT actions_metadata FROM webdock_auth.plugin_source_import WHERE id=$1",
            [expired.id],
          )
        ).rows[0].actions_metadata,
      ).toBeNull();
      await expect(
        actionsRead({ ...actionsInput, id: expired.id }, actionsClaims),
      ).rejects.toThrow();
      const unsupported = await command({
        action: "create",
        projectID: project,
        source: { kind: "git", url: "https://example.com/repo" },
        mode: "build",
        idempotencyKey: "unsupported",
      });
      const unsupportedClaim = await workerCall("claim", {});
      await workerCall("complete", {
        id: unsupported.id,
        claimToken: unsupportedClaim.claimToken,
        result: {
          phase: "build",
          manifestRaw: JSON.stringify({
            ...manifest,
            jobs: [{ name: "job", capabilities: [] }],
          }),
          sourceSHA256: "b".repeat(64),
        },
      });
      const failed = (
        await command({ action: "list", projectID: project })
      ).imports.find((j: any) => j.id === unsupported.id);
      expect(failed.status).toBe("failed");
      expect(failed.error).toContain("full lifecycle controller");
      const stale = await command({
        action: "create",
        projectID: project,
        source: { kind: "git", url: "https://example.com/repo" },
        mode: "build",
        idempotencyKey: "stale",
      });
      const staleClaim = await workerCall("claim", {}),
        newRaw = JSON.stringify({ ...manifest, name: plugin + "-stale" });
      await workerCall("complete", {
        id: stale.id,
        claimToken: staleClaim.claimToken,
        result: {
          phase: "build",
          manifestRaw: newRaw,
          sourceSHA256: "c".repeat(64),
        },
      });
      await command({
        ...approval,
        id: stale.id,
        manifestSha256: sourceHash(newRaw),
      });
      await owner.query(
        "UPDATE webdock_auth.plugin_source_import SET expires_at=now()-interval '1 second' WHERE id=$1",
        [stale.id],
      );
      expect(await workerCall("claim", {})).toBeNull();
      await expect(
        command({ action: "activate", id: stale.id }),
      ).rejects.toThrow();
      expect(
        (
          await owner.query(
            "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
            [plugin + "-stale"],
          )
        ).rows,
      ).toEqual([]);
      const revoked = await command({
        action: "create",
        projectID: project,
        source: { kind: "git", url: "https://example.com/repo" },
        mode: "build",
        idempotencyKey: "revoked",
      });
      const revokedClaim = await workerCall("claim", {});
      await workerCall("actions-prepare", {
        ...prepared,
        id: revoked.id,
        claimToken: revokedClaim.claimToken,
      });
      await workerCall("actions-chunk", {
        id: revoked.id,
        claimToken: revokedClaim.claimToken,
        offset: 0,
        data: preparedBytes.toString("base64"),
      });
      await workerCall("actions-bind", {
        id: revoked.id,
        claimToken: revokedClaim.claimToken,
        runID: "456",
      });
      await owner.query(
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        [users[0]],
      );
      await expect(
        actionsRead({ ...actionsInput, id: revoked.id }, actionsClaims),
      ).rejects.toMatchObject({ status: 403, code: "authority_revoked" });
      expect(
        await workerCall("lease", {
          id: revoked.id,
          claimToken: revokedClaim.claimToken,
        }),
      ).toEqual({ active: false });
      await owner.query(
        "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
        [users[0]],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_source_import(actor_id,actor_scope,project_id,customer_id,source_kind,source_label,status,idempotency_key,request_sha256) SELECT $1,'{}',$2,$3,'git','Global limit fixture','queued','global-'||n,repeat('a',64) FROM generate_series(1,32) n",
        [users[1], project, customer],
      );
      await expect(command(zip("global-over-limit"))).rejects.toThrow(
        "32 total",
      );
    } catch (error) {
      if (databaseFailure) throw Error(databaseFailure);
      throw error;
    } finally {
      await owner.query(
        "DELETE FROM webdock_auth.plugin_source_import WHERE actor_id=ANY($1)",
        [users],
      );
      for (const table of [
        "plugin_grant",
        "plugin_runtime",
        "plugin_configuration",
        "plugin_release",
        "plugin_installation",
      ])
        await owner.query(
          `DELETE FROM webdock_auth.${table} WHERE ${table === "plugin_installation" ? "id" : "plugin_id"}=ANY($1)`,
          [installedNames],
        );
      await owner.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
        project,
      ]);
      const organizations = (
        await owner.query(
          "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",
          [customer],
        )
      ).rows.map((row) => row.organization_id);
      await owner.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
        customer,
      ]);
      await owner.query(
        "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
        [organizations],
      );
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
        users,
      ]);
      await api.end();
      await worker.end();
      await owner.end();
    }
  },
  30000,
);
