# Registry protocol admission preparation

This directory prepares local schema artifacts and checks. It never contacts the
registry, publishes images/releases, generates publisher keys, or activates a
provider binding.

## Maintained Tabularium contract

The reviewed Tabularium source now supports an explicit `kind.manifestSchema`
field persisted in its existing `plugin_kinds` settings JSON. No database
migration is required. It accepts one complete closed kind object or a bounded
`oneOf` of complete closed kind objects. Every branch requires the exact kind.
The locked Tabularium name/version/metadata constraints remain in force.

The new validator traverses all supported schema containers, rejects references
and unsupported keywords, and bounds schema bytes, depth, nodes, branches,
properties, patterns and annotation lengths. Kind-specific and kindless schema
discovery, strict authoring and real source/release/refresh ingestion use the same
root contract. Ingestion for a configured root kind rejects unknown fields instead
of stripping them. Legacy kinds without this field retain their previous behavior.
Metadata-only kind updates preserve an existing root policy; clearing it requires
an explicit null. An invalid stored root policy fails closed. A four-entry,
size-bounded content cache avoids repeatedly compiling complete schema unions.
Repeated schema subtrees are shared through generated local `$defs` only after
the raw policies are validated and composed. Those generated references are
acyclic, never resolve outside the document, and never rewrite `const`/default
data. Operator-supplied references remain forbidden. AJV compiles these shared
definitions once; the runtime also uses Bun's supported `--smol` collection mode.
This avoids the very large duplicate validator functions that exceeded the
existing 512 MiB budget. Diagnostic budget evidence is explicitly marked
non-promotable; the final signed image needs its own qualification.

`extensionsSchema` still uses its original property-delta contract. Pasting a
root union there silently discards root constraints; widening independent protocol
fields to `[1,2]` accepts mixed tuples. Regression tests retain this proof. Use the
new explicit field, not a per-field widening or a new `native-v2` kind.

## Extension v1/v2 union

```sh
TABULARIUM_SOURCE_ROOT=/absolute/reviewed/tabularium \
  bun --no-env-file test tools/plugin-registry

bun --no-env-file tools/plugin-registry/prepare.ts \
  /absolute/reviewed/tabularium tools/plugins/tabularium-kind.schema.json \
  > /private/reviewed-extension-schema-preparation.json
```

Preparation reads local source only, records source/schema hashes and emits the
`manifestSchema` field to merge into the exact authenticated current `extension`
kind. Existing kind text, translations, display settings and `extensionsSchema`
must be retained. It emits no write command. Old source without explicit root
support returns `status: blocked`, `patch: null`, exit 2. Supported source returns
`status: prepared`, exit 0. This does not attest a live registry deployment.

The union preserves existing v1 field constraints and adds the closed native-v2
structure: exact 2/2/2 tuple, audience, offerings, features, areas, bounded
capabilities/services/workloads, workload-capability relationship, read-action
capabilities and resource/storage conditions. Supported capabilities and workload
kinds come from the current authoritative Rust constants. Existing v1 fields and
three-level bounded input/configuration descriptions are reused. Five existing
external v1 release manifests and all six root native product manifests are tested
against both discovery forms, plus invalid privilege, tuple, image, array, map and
depth cases. Native images must have real immutable nonzero digests.

JSON Schema is the structural registry contract. Dynamic relationships such as
unique IDs by field, foreign-key targets, feature/action references, runtime
binding admission and semantic version-range parsing remain authoritative Core
parser checks during signed release preflight/installation. The local Rust helper
runs those checks; registry schema validation alone is never installation evidence.

## Protocol provider catalog

`provider-kind.json` is the generated complete admin-kind artifact from
`provider-kind.ts`. It uses kind **provider**, with exact protocol branches
`webdock.repository.v1` and `webdock.mail.v1`. The existing locked registry base
fields are compatible. Both branches require nonzero immutable OCI identity,
immutable GitHub `sourceRepo` plus 40-hex `revision`, `catalogOnly: true` and explicit
runtime-binding/authenticity metadata. Generic extension installation is always
false; Core's extension parser rejects this kind.

A published catalog entry may truthfully use:

```json
{
  "status": "required-before-activation",
  "protocol": "webdock.repository.v1",
  "authenticity": "publisher-signed-descriptor",
  "genericExtensionInstaller": false
}
```

This is the `runtimeBinding` object, not a full release manifest. The Mail branch
uses protocol `webdock.mail.v1` and authenticity `core-descriptor-hash`. Pending
entries must omit descriptor and transport fields; no fake hashes, signing keys
or installed binding are needed to publish their real OCI artifacts.

Only a configured binding may carry descriptor metadata, and it must supply the
protocol-specific authentication evidence and exact peer transport kind.
Repository providers require the signed-descriptor hash, publisher key hash,
signer key ID, domain, separately admitted installation-manifest hash and image
binding. Mail requires the Core-configured descriptor hash and signed lease match;
no publisher signature is invented. Runtime keys, socket paths, credentials and
private network addresses never belong in catalog metadata. Repository descriptor
signing binds a **separate Core-admitted installation manifest**; embedding its
signature hash into the manifest it signs would create a circular hash dependency.
Catalog metadata is not a cryptographic verification or activation receipt.

Regenerate without touching the registry:

```sh
bun --no-env-file tools/plugin-registry/provider-kind.ts \
  > tools/plugin-registry/provider-kind.json
```

## Rollout order

1. Review/build the modified Tabularium manifest package and API in the existing
   EU registry deployment workflow; retain settings, signing/OAuth keys, database
   and persistent storage. Source checks alone do not deploy this contract.
2. Fetch the authenticated current extension kind and preserve its exact fields.
   Regenerate/compare the v1 baseline, then merge only prepared `manifestSchema`.
   Create the separate provider kind from its reviewed artifact if absent; an
   existing kind must be reconciled, not overwritten blindly.
3. Verify specific and kindless schema discovery, actual existing v1 releases,
   finalized native manifests, provider catalog manifests, and negative cases
   through the live authoring endpoint before any release publication.
4. Verify signed universal descriptor ingestion separately from OCI publication.
   Installation and provider runtime binding are subsequent explicit operations.
   Do not roll the API back to a build ignoring root policies while relying on
   those policies for newly published v2/provider entries.

## Native release preflight

```sh
cargo test --offline --locked \
  --manifest-path tools/plugin-registry/manifest-validator/Cargo.toml \
  --target-dir target

cargo run --offline --locked --quiet \
  --manifest-path tools/plugin-registry/manifest-validator/Cargo.toml \
  --target-dir target < finalized-manifest.json
```

The helper reads at most 64 KiB plus one sentinel byte, uses the authoritative
`webdock_plugin_protocol::Manifest::parse`, rejects zero OCI digests and prints
exact original bytes on success. Errors contain no manifest data. `--authoring`
allows the zero digest for pre-build checks only; no other rule is relaxed.

Exported plugin repositories copy it to `tools/manifest-validator`, pin its protocol
dependency to the same immutable SDK Git revision and regenerate its lockfile.
The Rust fixture tests use the local Core checkout's product manifests; production
validation uses only stdin and the pinned protocol crate.
