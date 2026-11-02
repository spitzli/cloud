# Agent control

This private Core crate implements the generated `AgentControl` gRPC service against retained hosting operations and restricted PostgreSQL roles. It does not execute Docker commands or receive Kubernetes credentials. Agents initiate their connections.

`ControlPlane::new(authentication_database, hosting_service, local_binding)` verifies the restricted authentication role. A local listener has one explicit installation/agent/UID binding. Remote listeners require mutual TLS. The shared protocol transport helper reads Tonic's verified connection extensions and checks certificate validity on every call. Enrollment binds the transport to the current retained cluster credential hash and generation; credential rotation and transport revocation invalidate established channels too. Caller-supplied certificate metadata never authenticates a request.

Apply the enrollment migration offline after the hosting runtime migration. Enrollment creation, transport/key replacement and revocation are deliberately unavailable to the runtime database role. Its enrollment updates are restricted to liveness fields. The compound cluster/installation foreign key prevents an enrollment from targeting a different installation. The original bearer hash remains authoritative during transport migration; no raw bearer credential is stored here.

Poll derives a typed command through the hosting transaction and persists the dispatch before returning it. A lost Poll response redelivers the same live dispatch. Guard rechecks the current operation lease, user authority, enrollment generation, exact command fingerprint and authorization MAC. Completion uses the durable dispatch and records the exact receipt hash; repeated successful receipts are acknowledged without repeating the mutation. Runtime failure receipts use the hosting domain's atomic failure marker. Interrupted execution requires reconciliation, never blind retries under a different resource identity.

A connectivity-only heartbeat advances only transport liveness. Capacity freshness changes only after a validated full runtime observation. Generations and sequences cannot be selected from observation JSON in place of the protocol envelope.

The executable is part of the Core package:

```sh
cargo run -p webdock-control-plane --bin webdock-agent-control -- --config /absolute/private/control.json
```

It uses the same explicit restricted database environment as the hosting built-in; it neither loads environment files nor migrates at startup. `WEBDOCK_FEATURES` must permit accepted work for a configured Hosting, Databases or incoming Mail domain. Incoming Mail additionally requires the explicit native-agent flag and reviewed image/hostname/retained-key configuration documented in the Core README. Keep the configuration file mode 0600. Local configuration:

```json
{"kind":"unix","path":"/run/webdock/private/control.sock","installation":"webdock","agent_id":"local-agent","uid":1000}
```

The directory must already be canonical, privately owned and mode 0700. The UID must match the explicit enrollment; it is not inferred from JSON supplied by an agent. The socket inode owner removes only the socket it created at shutdown. Remote configuration explicitly selects `kind: "mtls"`, `address`, and absolute private `certificate`, `key`, `peer_ca` paths. There is no plaintext TCP mode. Certificate issuance/enrollment CLI and final distribution wiring remain separate acceptance gates.

Real restricted PostgreSQL and socket tests cover Unix and mTLS registration, protocol/identity denial, same-channel revocation, changed generation, runtime inability to edit its own enrollment and connectivity-only heartbeat freshness. Full persisted dispatch → daemon → runtime testing is being integrated. Retained Kubernetes command conversion, native read-command/log/metrics dispatch and all resource capability gates must be complete before retiring the old executor.
