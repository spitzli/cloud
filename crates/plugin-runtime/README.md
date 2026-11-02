# Plugin runtime transport

This crate hosts the versioned `webdock.plugin.v1.PluginRuntime` gRPC service. It does not start a process, install a plugin, access a database, or provide platform host services.

Construct `Runtime::new(manifest, generation, verifying_key, authority, handler)`. Both `InvocationAuthority` methods deny by default. A production authority must map the authenticated peer to its enrolled identity and revalidate the persisted invocation, session, installation state, exact runtime/configuration generation, project ownership, capability approvals and grant revision. Revalidation happens before execution, every 250 ms while pending, and before returning results. Certificate revocation on an established connection must be checked by this authority too.

`ActionHandler::invoke` receives verified claims and schema-validated input/configuration. Its future is dropped on client cancellation, authorization failure, expiry or the 30-second execution limit. A dropped future cannot undo an already accepted external operation: durable operation IDs, receipts and fencing remain the host's responsibility. Handler errors are mapped to stable gRPC statuses without forwarding potentially secret diagnostic strings.

For local execution, create a canonical absolute directory owned by the current UID with mode 0700. `UnixSocket::bind` creates a mode-0600 socket, refuses to replace existing paths, and removes only its own inode on shutdown. `serve_unix` verifies the connecting UID; `connect_unix` checks directory and socket ownership and permissions. This mode assumes each installation's runtime processes share the configured OS UID and that ancestors of the private directory are trusted. It creates no TCP management listener.

Remote execution is explicit: `serve_mtls` accepts an already-bound TCP listener and `MutualTls { identity, peer_ca }`. Client certificates are mandatory. `connect_mtls` requires HTTPS, a server name, client identity and peer CA. Tonic/Rustls implement TLS and HTTP/2. Peer certificate bytes are available to the authority for enrollment and revocation checks; this crate does not create an enrollment database or certificate rotation controller.

`Describe` returns exact original manifest bytes, including whitespace, so release digest/signature verification remains possible. `Health` returns the plugin ID, version, generation and protocol version after peer authorization. `Invoke` binds the requested action to the signed token, caps JSON input at 32 KiB, configuration/output at 64 KiB, and bounds concurrent executions at 32. All RPCs require peer authorization. `WatchOperation` is reserved but returns `Unimplemented`: there is no fake operation store or event stream.

Build requires `protoc`; generated Rust bindings are build outputs, while `proto/webdock/plugin/v1/plugin.proto` is the language-neutral source contract. Tests use real Unix and loopback TLS sockets, disposable certificates and a test-only echo handler.

Not implemented here: legacy HTTP serving, registry/source installation, host apps/data/HTTP/jobs adapters, operation persistence/streaming, runtime supervision, declarative UI and provider implementations. Existing HTTP plugins need a separately reviewed migration adapter; the exact legacy Ed25519 token format is already preserved and covered by a Node-generated fixture.
