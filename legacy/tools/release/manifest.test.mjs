import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile, cp, rm, mkdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createManifest,
  verifyManifest,
  validateVersion,
  validateEvidence,
  validateCMSPlugin,
  images,
  databaseImages,
  databaseExecutionChecks,
  executionChecks,
  inspectOCIArchive,
} from "./manifest.mjs";
const revision = "a".repeat(40);
const evidence = {
  schemaVersion: 1,
  revision,
  platform: "linux/amd64",
  runnerPlatform: "linux/amd64",
  emulated: false,
  checks: executionChecks.map((name) => ({ name, passed: true })),
  imageIndexDigests: Object.fromEntries(
    images.map((name) => [name, "sha256:" + "0".repeat(64)]),
  ),
};
async function fixture(dir, { nested = false, multipleRoots = false } = {}) {
  const root = join(dir, "oci");
  await mkdir(join(root, "blobs/sha256"), { recursive: true });
  const blob = async (value, mediaType) => {
    const data = Buffer.from(JSON.stringify(value)),
      hash = createHash("sha256").update(data).digest("hex");
    await writeFile(join(root, "blobs/sha256", hash), data);
    return { mediaType, digest: "sha256:" + hash, size: data.length };
  };
  const config = await blob(
    { architecture: "amd64", os: "linux" },
    "application/vnd.oci.image.config.v1+json",
  );
  const manifest = await blob(
    { schemaVersion: 2, config, layers: [] },
    "application/vnd.oci.image.manifest.v1+json",
  );
  const image = { ...manifest, platform: { os: "linux", architecture: "amd64" } };
  const registryRoot = nested
    ? await blob({ schemaVersion: 2, manifests: [image] }, "application/vnd.oci.image.index.v1+json")
    : image;
  await writeFile(
    join(root, "index.json"),
    JSON.stringify({
      schemaVersion: 2,
      manifests: multipleRoots ? [registryRoot, image] : [registryRoot],
    }),
  );
  for (const name of images)
    execFileSync("tar", [
      "-cf",
      join(dir, name + ".oci.tar"),
      "-C",
      root,
      "index.json",
      "blobs",
    ]);
  for (const name of ["webdock", "webdock-agent"])
    await writeFile(join(dir, `${name}-0.1.0.tgz`), name);
  const catalogueDigest = "sha256:" + createHash("sha256")
    .update(await readFile(join(root, "index.json"))).digest("hex");
  await writeFile(
    join(dir, "architecture-evidence.json"),
    JSON.stringify({
      ...evidence,
      imageIndexDigests: Object.fromEntries(images.map(name => [name, catalogueDigest])),
    }),
  );
  return registryRoot.digest;
}
test("release OCI identities, proof and checksums round-trip; tampering fails", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-"));
  try {
    assert.throws(() => validateVersion("../bad"));
    await fixture(dir);
    const manifest = await createManifest(dir, "0.1.0", revision);
    assert.equal((await verifyManifest(dir)).artifacts.length, 11);
    for (const name of ["api", "console", "api-core", "console-core"])
      assert.ok(manifest.artifacts.some((artifact) => artifact.file === `${name}.oci.tar`));
    assert.match(
      manifest.artifacts.find((a) => a.kind === "oci-image").oci.indexDigest,
      /^sha256:/,
    );
    await writeFile(join(dir, "api.oci.tar"), "tampered");
    await assert.rejects(verifyManifest(dir), /mismatch/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test("unsupported or unexecuted architecture, missing checks, artifacts and proof cannot release", async () => {
  for (const bad of [
    { ...evidence, emulated: true },
    { ...evidence, runnerPlatform: "linux/arm64" },
    { ...evidence, checks: [] },
    { ...evidence, revision: "b".repeat(40) },
  ])
    assert.throws(() => validateEvidence(bad, revision));
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-"));
  try {
    await fixture(dir);
    await rm(join(dir, "architecture-evidence.json"));
    await assert.rejects(createManifest(dir, "0.1.0", revision));
    await writeFile(
      join(dir, "architecture-evidence.json"),
      JSON.stringify({
        ...evidence,
        platform: "linux/arm64",
        runnerPlatform: "linux/arm64",
      }),
    );
    await assert.rejects(
      createManifest(dir, "0.1.0", revision),
      /architecture/,
    );
    await fixture(dir);
    await rm(join(dir, "api.oci.tar"));
    await assert.rejects(createManifest(dir, "0.1.0", revision), /Missing api/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
test("manifest cannot add unsupported architecture or substitute OCI digest", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-"));
  try {
    await fixture(dir);
    const manifest = await createManifest(dir, "0.1.0", revision);
    manifest.supportedArchitectures.push("linux/arm64");
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /architecture/);
    manifest.supportedArchitectures.pop();
    manifest.artifacts[0].oci.indexDigest = "sha256:" + "0".repeat(64);
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /OCI/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("unlisted stale archives cannot be published beside a verified manifest", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-extra-"));
  try {
    await fixture(dir);
    await createManifest(dir, "0.1.0", revision);
    for (const name of ["database-gateway.oci.tar", "unreviewed-0.1.0.tgz"]) {
      await writeFile(join(dir, name), "not covered by signed checksums");
      await assert.rejects(verifyManifest(dir), /Unexpected archive outside manifest/);
      await rm(join(dir, name));
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("optional database artifacts require pinned source, gateway execution proof and matching chart", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-database-release-"));
  try {
    await fixture(dir);
    const proof = JSON.parse(
      await readFile(join(dir, "architecture-evidence.json"), "utf8"),
    );
    for (const name of databaseImages) {
      await cp(join(dir, "api.oci.tar"), join(dir, name + ".oci.tar"));
      proof.imageIndexDigests[name] = inspectOCIArchive(
        join(dir, name + ".oci.tar"),
      ).indexDigest;
    }
    await writeFile(
      join(dir, "architecture-evidence.json"),
      JSON.stringify(proof),
    );
    await assert.rejects(
      createManifest(dir, "0.1.0", revision),
      /Unexpected artifact/,
    );
    proof.databaseRuntime = {
      sourceCommit: "b".repeat(40),
      sourceTree: "c".repeat(40),
      sourceImage: "registry.example/tabularis@sha256:" + "d".repeat(64),
    };
    await writeFile(
      join(dir, "architecture-evidence.json"),
      JSON.stringify(proof),
    );
    await assert.rejects(
      createManifest(dir, "0.1.0", revision),
      /execution check/,
    );
    proof.checks.push(
      ...databaseExecutionChecks.map((name) => ({ name, passed: true })),
    );
    await writeFile(
      join(dir, "architecture-evidence.json"),
      JSON.stringify(proof),
    );
    await assert.rejects(
      createManifest(dir, "0.1.0", revision),
      /Missing webdock-databases chart/,
    );
    await writeFile(join(dir, "webdock-databases-0.1.0.tgz"), "chart fixture");
    const manifest = await createManifest(dir, "0.1.0", revision);
    assert.equal(
      manifest.databaseRuntime.sourceCommit,
      proof.databaseRuntime.sourceCommit,
    );
    assert.equal((await verifyManifest(dir)).artifacts.length, 14);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("optional CMS release requires both tested images, pinned versions and signed plugin metadata", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-cms-release-"));
  try {
    await fixture(dir, { nested: true });
    const proof = JSON.parse(await readFile(join(dir, "architecture-evidence.json"), "utf8"));
    proof.imageRegistryDigests = Object.fromEntries(images.map(name => [name, inspectOCIArchive(join(dir, `${name}.oci.tar`)).registryDigest]));
    proof.cmsRuntime = { payloadVersion: "4.0.0-canary.37", pluginVersion: "0.1.0-alpha.1" };
    proof.checks.push(...["cms-instance-fresh-postgres", "managed-cms-runtime"].map(name => ({ name, passed: true })));
    for (const name of ["cms-instance", "managed-cms"]) {
      await cp(join(dir, "api.oci.tar"), join(dir, `${name}.oci.tar`));
      const oci = inspectOCIArchive(join(dir, `${name}.oci.tar`));
      proof.imageIndexDigests[name] = oci.indexDigest;
      proof.imageRegistryDigests[name] = oci.registryDigest;
    }
    await writeFile(join(dir, "architecture-evidence.json"), JSON.stringify(proof));
    await assert.rejects(createManifest(dir, "0.1.0", revision), /Missing managed-cms.plugin.json/);
    const plugin = { name: "managed-cms", version: "0.1.0-alpha.1", kind: "extension", protocolVersion: 1, sdkVersion: 1, schemaVersion: 1, image: `ghcr.io/spitzli/webdock/managed-cms@${proof.imageRegistryDigests["managed-cms"]}` };
    assert.equal(validateCMSPlugin(plugin, proof, "ghcr.io/spitzli/webdock"), plugin);
    assert.throws(() => validateCMSPlugin(plugin, proof, "registry.example/webdock"), /destination image/);
    await writeFile(join(dir, "managed-cms.plugin.json"), JSON.stringify(plugin));
    const manifest = await createManifest(dir, "0.1.0", revision);
    assert.equal(manifest.artifacts.find(a => a.file === "managed-cms.plugin.json").kind, "plugin-manifest");
    assert.match(await readFile(join(dir, "SHA256SUMS"), "utf8"), /  managed-cms.plugin.json\n/);
    assert.equal((await verifyManifest(dir)).artifacts.length, 14);
    const companion = manifest.artifacts.find(a => a.file === "managed-cms.plugin.json");
    companion.kind = "helm-chart";
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /artifact kind/);
    companion.kind = "plugin-manifest";
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await rm(join(dir, "managed-cms.plugin.json"));
    await assert.rejects(verifyManifest(dir), /CMS plugin metadata/);
    for (const invalid of [null, {}, { ...plugin, version: "0.2.0" }, { ...plugin, image: "ghcr.io/spitzli/webdock/managed-cms:latest" }, { ...plugin, image: `ghcr.io/spitzli/webdock/managed-cms@sha256:${"f".repeat(64)}` }]) {
      await writeFile(join(dir, "managed-cms.plugin.json"), JSON.stringify(invalid));
      await assert.rejects(createManifest(dir, "0.1.0", revision), /CMS plugin metadata/);
    }
    await writeFile(join(dir, "managed-cms.plugin.json"), "{");
    await assert.rejects(createManifest(dir, "0.1.0", revision), /CMS plugin metadata/);
    assert.throws(() => validateEvidence({ ...proof, imageRegistryDigests: undefined }, revision), /registry digest evidence/);
    assert.throws(() => validateEvidence({ ...proof, cmsRuntime: { ...proof.cmsRuntime, payloadVersion: "latest" } }, revision), /pinned CMS/);
    assert.throws(() => validateEvidence({ ...proof, checks: evidence.checks }, revision), /execution check/);
    const { ["managed-cms"]: ignored, ...incomplete } = proof.imageIndexDigests;
    assert.throws(() => validateEvidence({ ...proof, imageIndexDigests: incomplete }, revision), /digest evidence/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("non-CMS releases reject stray plugin metadata", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-cms-stray-"));
  try {
    await fixture(dir);
    await createManifest(dir, "0.1.0", revision);
    await writeFile(join(dir, "managed-cms.plugin.json"), "{}");
    await assert.rejects(verifyManifest(dir), /Unexpected artifact outside manifest/);
    await assert.rejects(createManifest(dir, "0.1.0", revision), /Unexpected artifact/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("nested OCI layout catalogue digest differs from its validated registry root", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-root-"));
  try {
    const expected = await fixture(dir, { nested: true });
    const oci = inspectOCIArchive(join(dir, "api.oci.tar"));
    assert.notEqual(oci.indexDigest, expected);
    assert.equal(oci.registryDigest, expected);
    const proof = JSON.parse(await readFile(join(dir, "architecture-evidence.json"), "utf8"));
    proof.imageRegistryDigests = Object.fromEntries(images.map(name => [name, expected]));
    await writeFile(join(dir, "architecture-evidence.json"), JSON.stringify(proof));
    const manifest = await createManifest(dir, "0.1.0", revision);
    assert.equal((await verifyManifest(dir)).artifacts[0].oci.registryDigest, expected);
    const artifact = manifest.artifacts.find(item => item.file === "api.oci.tar");
    artifact.oci.registryDigest = "sha256:" + "f".repeat(64);
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /OCI/);
    delete artifact.oci.registryDigest;
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /OCI/);
    artifact.oci.registryDigest = expected;
    proof.imageRegistryDigests.api = "sha256:" + "f".repeat(64);
    await writeFile(join(dir, "architecture-evidence.json"), JSON.stringify(proof));
    await assert.rejects(createManifest(dir, "0.1.0", revision), /execution evidence/);
    manifest.architectureEvidence = proof;
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    await assert.rejects(verifyManifest(dir), /OCI/);
    const { api: ignored, ...missing } = proof.imageRegistryDigests;
    assert.throws(() => validateEvidence({ ...proof, imageRegistryDigests: missing }, revision), /registry digest evidence/);
    const root = join(dir, "oci");
    const blob = join(root, "blobs/sha256", expected.slice(7));
    await writeFile(blob, (await readFile(blob, "utf8")) + " ");
    execFileSync("tar", ["-cf", join(dir, "api.oci.tar"), "-C", root, "index.json", "blobs"]);
    assert.throws(() => inspectOCIArchive(join(dir, "api.oci.tar")), /OCI blob digest mismatch/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("CMS companion requires the publishable registry root, never the layout catalogue digest", () => {
  const proof = {
    ...evidence,
    cmsRuntime: { payloadVersion: "4.0.0-canary.37", pluginVersion: "0.1.0-alpha.1" },
    imageIndexDigests: { ...evidence.imageIndexDigests, "managed-cms": "sha256:" + "a".repeat(64) },
    imageRegistryDigests: { "managed-cms": "sha256:" + "b".repeat(64) },
  };
  const plugin = { name: "managed-cms", version: "0.1.0-alpha.1", kind: "extension", protocolVersion: 1, sdkVersion: 1, schemaVersion: 1, image: `ghcr.io/spitzli/webdock/managed-cms@${proof.imageRegistryDigests["managed-cms"]}` };
  assert.equal(validateCMSPlugin(plugin, proof, "ghcr.io/spitzli/webdock"), plugin);
  assert.throws(() => validateCMSPlugin({ ...plugin, image: `ghcr.io/spitzli/webdock/managed-cms@${proof.imageIndexDigests["managed-cms"]}` }, proof), /CMS plugin metadata/);
  assert.throws(() => validateCMSPlugin(plugin, { ...proof, imageRegistryDigests: undefined }), /CMS plugin metadata/);
});

test("ambiguous multi-root OCI layouts cannot be selected implicitly for publication", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-roots-"));
  try {
    await fixture(dir, { nested: true, multipleRoots: true });
    assert.throws(() => inspectOCIArchive(join(dir, "api.oci.tar")), /single registry root/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test("legacy non-CMS signed manifests without registry metadata still verify", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-release-legacy-"));
  try {
    await fixture(dir, { nested: true });
    const manifest = await createManifest(dir, "0.1.0", revision);
    for (const artifact of manifest.artifacts) if (artifact.oci) delete artifact.oci.registryDigest;
    await writeFile(join(dir, "manifest.json"), JSON.stringify(manifest));
    assert.equal((await verifyManifest(dir)).artifacts.length, 11);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
