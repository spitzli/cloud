# Linux amd64 only. Build the Rust musl runner separately, then copy it into this context.
FROM docker.io/moby/buildkit:v0.34.1-rootless@sha256:f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89 AS release-tools
USER 0
ARG TARGETARCH
RUN test "$TARGETARCH" = amd64
ADD --checksum=sha256:2842de93e68713af9037e0bc0a398d7da78f3b96aa4804303a638db2bc69bd30 https://github.com/railwayapp/railpack/releases/download/v0.40.1/railpack-v0.40.1-x86_64-unknown-linux-musl.tar.gz /tmp/railpack.tar.gz
ADD --checksum=sha256:e331f3fa3c360c63e996ef70bc183ce178cb16d21f8bf4926c61cf6fa6b9fafd https://github.com/jdx/mise/releases/download/v2026.9.15/mise-v2026.9.15-linux-x64-musl.tar.gz /tmp/mise.tar.gz
RUN mkdir -p /release /tmp/railpack-release /tmp/mise-release \
 && tar -xzf /tmp/railpack.tar.gz -C /tmp/railpack-release \
 && tar -xzf /tmp/mise.tar.gz -C /tmp/mise-release \
 && cp /tmp/railpack-release/railpack /release/railpack \
 && cp /tmp/mise-release/mise/bin/mise /release/mise-2026.9.15 \
 && printf '%s\n' '6c99bdd6d9606261a218fb22f3d663eb72fc483e8dc8c839844c15aa0c2fbdf1  /release/railpack' 'b46c551f5e795a81144a3dbe74227c40b7cbe231024d8e9167e691208031ec30  /release/mise-2026.9.15' | sha256sum -c - \
 && chmod 0755 /release/railpack /release/mise-2026.9.15

FROM docker.io/moby/buildkit:v0.34.1-rootless@sha256:f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89
USER 0
COPY --from=release-tools /release/railpack /usr/local/bin/railpack
COPY --from=release-tools /release/mise-2026.9.15 /usr/local/libexec/mise-2026.9.15
COPY spitzli-build-runner /usr/local/bin/spitzli-build-runner
COPY entrypoint.sh /usr/local/bin/webdock-build-entrypoint
COPY mise-wrapper.sh /usr/local/bin/webdock-mise-wrapper
COPY daemon-logger.sh /usr/local/bin/webdock-build-daemon-logger
COPY tools.json /etc/webdock-build/tools.json
COPY --chmod=0444 transfer-policy.json /etc/webdock-build/transfer-policy.json
ARG RUST_RUNNER_SHA256
RUN test -n "$RUST_RUNNER_SHA256" \
 && printf '%s  /usr/local/bin/spitzli-build-runner\n' "$RUST_RUNNER_SHA256" | sha256sum -c - \
 && chmod 0755 /usr/local/bin/spitzli-build-runner /usr/local/bin/webdock-build-entrypoint /usr/local/bin/webdock-mise-wrapper /usr/local/bin/webdock-build-daemon-logger \
 && chmod 0644 /etc/webdock-build/tools.json \
 && mkdir -p /handoff \
 && touch /run/.containerenv
ENV TMPDIR=/workspace XDG_RUNTIME_DIR=/workspace/runtime
USER 1000:1000
# Verify actual image permissions; controller policy files are deliberately private before COPY.
RUN test -s /etc/webdock-build/transfer-policy.json \
 && head -c 1 /etc/webdock-build/transfer-policy.json >/dev/null \
 && test -r /etc/webdock-build/tools.json \
 && test -x /usr/local/bin/spitzli-build-runner \
 && test -x /usr/local/bin/webdock-mise-wrapper
ENTRYPOINT ["/usr/local/bin/webdock-build-entrypoint"]
