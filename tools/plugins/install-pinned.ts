import {preservedConfigurationRevision} from "./update-policy";
/** Explicit offline operator installation. Release bytes must have passed pinned-registry verification. */
import { readFileSync, statSync } from "node:fs";
import { Pool } from "pg";
import { z } from "zod";
import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import { verifyRelease } from "../../adapters/plugins/tabularium/src/integrity";
const file = process.argv[2],
  target = process.argv[3];
if (!file || statSync(file).mode & 0o077)
  throw Error("Private installation input required");
const config = JSON.parse(readFileSync(file, "utf8"));
const release = await verifyRelease(config.release);
if (
  release.manifest.dependencies.length ||
  release.manifest.tables.length ||
  release.manifest.migrations.length ||
  release.manifest.jobs.length
)
  throw Error(
    "This operator pilot supports stateless action runtimes with scoped documents; unsupported lifecycle obligations",
  );
const url = new URL(process.env.DATABASE_URL_UNPOOLED ?? "");
if (
  `${url.hostname}:${url.port || 5432}${url.pathname}` !== target ||
  url.username !== "webdock_offline_migrate" ||
  !process.env.DATABASE_CA_CERT
)
  throw Error("Exact offline migration target and verified CA required");
const projectID = z
  .string()
  .regex(/^[0-9]{1,20}$/)
  .parse(config.projectID);
const customerID = z
  .string()
  .regex(/^[0-9]{1,20}$/)
  .parse(config.customerID);
const binding = z
  .strictObject({
    namespace: z.string().regex(/^[a-z0-9-]+$/),
    service: z.string().regex(/^[a-z0-9-]+$/),
    credentialHash: z.string().regex(/^[a-f0-9]{64}$/),
    clusterID: z.string().min(1),
    generation: z.number().int().positive().safe(),
  })
  .parse(config.runtime);
const approved = z.array(z.string()).parse(config.approvedCapabilities);
if (
  release.manifest.capabilities.some((c) => !approved.includes(c)) ||
  approved.some((c) => !release.manifest.capabilities.includes(c))
)
  throw Error("Explicit capabilities must match the reviewed release");
if (
  !z
    .fromJSONSchema(release.manifest.configuration as any)
    .safeParse(config.configuration).success
)
  throw Error("Invalid plugin configuration");
const db = new Pool({
  ...verifiedPostgresConfig(url.href, process.env.DATABASE_CA_CERT),
  max: 1,
});
const c = await db.connect();
try {
  await c.query("BEGIN");
  await c.query(
    "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'",
  );
  await c.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))",
  );
  const match = (
    await c.query(
      "SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND c.id=$2 AND p.status='active' AND c.status='active'",
      [projectID, customerID],
    )
  ).rows.length;
  if (!match) throw Error("Project/customer mismatch");
  const old = (
    await c.query(
      "SELECT * FROM webdock_auth.plugin_installation WHERE id=$1 FOR UPDATE",
      [release.id],
    )
  ).rows[0];
  let configurationRevision="0";
  if (old) {
    if (
      old.state !== "disabled" ||
      old.runtime !== "stopped" ||
      old.version !== config.previousVersion ||
      String(old.revision) !== config.expectedRevision ||
      binding.generation !== Number(old.current_generation) + 1
    )
      throw Error(
        "Review and stop the existing generation before staging an update",
      );
    if (
      (
        await c.query(
          "SELECT 1 FROM webdock_auth.plugin_invocation WHERE plugin_id=$1 AND revoked_at IS NULL AND expires_at>now()",
          [release.id],
        )
      ).rows.length
    )
      throw Error("Invocations are still draining");
    const previous = (
      await c.query(
        "SELECT revision::text,approved_capabilities,configuration FROM webdock_auth.plugin_configuration WHERE plugin_id=$1",
        [release.id],
      )
    ).rows[0];
    configurationRevision=preservedConfigurationRevision(previous,config.configuration,approved);
    await c.query(
      "UPDATE webdock_auth.plugin_installation SET previous_generation=current_generation,current_generation=$2,version=$3,image_digest=$4,runtime='starting',revision=revision+1,updated_at=now() WHERE id=$1",
      [release.id, binding.generation, release.version, release.image],
    );
  } else {
    if (binding.generation !== 1) throw Error("Initial generation must be 1");
    await c.query(
      "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,image_digest,runtime,current_generation) VALUES($1,'external','disabled','disabled',$2,$3,'starting',1)",
      [release.id, release.version, release.image],
    );
  }
  await c.query(
    "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,$2,$3,$4,$5,$6,$7)",
    [
      release.id,
      release.version,
      release.manifestBytes,
      release.manifestSha256,
      release.image,
      release.registryURL,
      release.signerKeyID,
    ],
  );
  if (!old)
    await c.query(
      "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES($1,$2::jsonb,$3::jsonb)",
      [
        release.id,
        JSON.stringify(approved),
        JSON.stringify(config.configuration),
      ],
    );
  await c.query(
    "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,cluster_id,namespace,service_name) VALUES($1,$8,$2,$3,'starting',$9,$4,$5,$6,$7)",
    [
      release.id,
      release.version,
      release.image,
      binding.credentialHash,
      binding.clusterID,
      binding.namespace,
      binding.service,
      binding.generation,
      configurationRevision,
    ],
  );
  if (!old)
    await c.query(
      "INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,$4::jsonb)",
      [release.id, customerID, projectID, JSON.stringify(approved)],
    );
  await c.query("COMMIT");
  console.log(
    JSON.stringify({
      installed: release.id,
      version: release.version,
      state: "disabled",
      ready: false,
      grantProject: projectID,
    }),
  );
} catch (e) {
  await c.query("ROLLBACK");
  throw e;
} finally {
  c.release();
  await db.end();
}
