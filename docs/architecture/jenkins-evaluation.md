# Jenkins evaluation for Webdock

Date: 2026-10-25. Status: engineering recommendation; no Jenkins deployment approved or performed.

## Decision

Keep the native isolated buildworker as Webdock's default. Treat Jenkins as a possible
external CI producer when a concrete customer already needs it. Do not add a shared
Jenkins controller to deliver the S3 scope. This recommendation does not implement
external CI admission, which remains unavailable until its provenance and authority
contract has been built and tested.

The repository already has durable build/release state, exact completion receipts,
current approval checks, isolated offline QEMU/KVM builds, OCI graph validation and
guarded publication. Jenkins would add another scheduler, credential store and
operational service without removing those responsibilities. There is currently no
owner requirement for a Jenkins-specific pipeline or integration that justifies that
extra service. See `crates/build/README.md` and `crates/build/src/{worker,service,publisher}.rs`.

## Operational and isolation cost

Jenkins' official guidance separates build agents from the controller and requires
agent-to-controller access control. Running customer build code on a different
process is not by itself a customer isolation boundary. If a future Jenkins service
executes mutually untrusted customer code, use independently isolated short-lived
agents and separately scoped identities; never mount the Docker socket, controller
home, Core configuration or deployment credentials into a customer job. Separate
controllers may be warranted for separate trust domains. This is an architectural
inference from the documented trust boundary, not a claim of tested Jenkins
multitenancy. [Controller isolation](https://www.jenkins.io/doc/book/security/controller-isolation/)

A production installation would require an accountable operator for controller/JDK
updates, an approved plugin inventory, advisory triage, recovery of controller state,
agent images and authentication/authorization configuration. Jenkins publishes
advisories for both Core and plugins, so a pinned deployment still needs active
maintenance. No current version or plugin set is qualified by this evaluation.
[Security advisories](https://www.jenkins.io/security/advisories/),
[security configuration](https://www.jenkins.io/doc/book/security/managing-security/)

Credential scope must follow the job's trust level. Any credential available to a
job must be considered accessible to its build code; masked logs are not an
exfiltration boundary. A future integration must therefore use restricted, expiring
handoff authority or a trusted external transfer process, with no durable Wasabi
keys in customer build scripts. [Credential guidance](https://www.jenkins.io/doc/book/security/credentials/)

## Artifact handoff and authority

A future external producer would submit an import request under a Webdock resource,
not create a successful Webdock release by uploading a file. The import must bind:

- customer, project, environment, source repository and immutable commit;
- producer identity, invocation/run identity, artifact digest and actual bytes;
- verified provenance, expected signatures/SBOM policy and the exact build generation;
- current Webdock approval, source revisions and the existing release operation.

Core's durable ledger remains the release truth. A trusted worker validates the
archive/OCI graph and copies approved bytes into its private artifact destination.
Webdock rechecks current authority before registry writes and deployment staging.
A Jenkins job result, Jenkins artifact URL, supplied commit label or possession of
an S3 URL is insufficient evidence. Replaying a prior Jenkins success must not
bypass current revocation or become an unconditional deployment instruction.

The existing Jenkins Artifact Manager on S3 supports artifact storage and configurable
S3 endpoints. It is useful for Jenkins' own artifact retention, but that is a separate
inventory from Webdock's retained release references. Its documented credentials and
agent transfer behavior must be qualified before use with Wasabi; the plugin page is
not a Wasabi compatibility or customer isolation test. Its deletion behavior must
never delete Webdock rollback targets. [Artifact Manager on S3](https://plugins.jenkins.io/artifact-manager-s3/)

## Reconsideration criteria

Revisit when an identified customer needs an existing Jenkins workflow, a required
integration is unavailable through the native worker/external CI contract, or a
measured operational benefit pays for the extra service. First define the import
contract above, then qualify a minimal plugin set, ephemeral agent isolation,
negative authority cases, lost responses and retained artifact recovery locally.
Keep provider storage, CI build execution and Webdock deployment authority distinct.

BuildKit, dependency, compiler and other build caches remain excluded, including
through Jenkins. This research installed no software, exposed no endpoint and used
no production credentials.
