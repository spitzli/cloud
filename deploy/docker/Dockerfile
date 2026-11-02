# Repository-root context. Core serves the built Console and account UI on their configured hosts.
FROM docker.io/oven/bun:1.4.3 AS console-build
WORKDIR /src
COPY . .
RUN bun install --frozen-lockfile --ignore-scripts
RUN bun run build:console

FROM docker.io/library/rust:1.98.1-slim-trixie AS rust-build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --locked --release -p webdock-control-plane --bins \
    && mkdir /binaries \
    && cp target/release/webdock-control-plane target/release/webdock-migrate \
       target/release/webdock-bootstrap-operator target/release/webdock-agent-control \
       target/release/webdock-plugin-control target/release/webdock-plugin-controller /binaries/

FROM docker.io/library/debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3t64 libzstd1 libbrotli1 zlib1g && rm -rf /var/lib/apt/lists/*
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
COPY --from=rust-build /binaries/ /usr/local/bin/
USER 65532:65532
ENV NODE_ENV=production
WORKDIR /opt/webdock

FROM runtime AS migrate
ENTRYPOINT ["/usr/local/bin/webdock-migrate"]

FROM runtime AS bootstrap-operator
ENTRYPOINT ["/usr/local/bin/webdock-bootstrap-operator"]

FROM runtime AS agent-control
ENTRYPOINT ["/usr/local/bin/webdock-agent-control"]

FROM runtime AS plugin-control
ENTRYPOINT ["/usr/local/bin/webdock-plugin-control"]

FROM runtime AS plugin-controller
ENTRYPOINT ["/usr/local/bin/webdock-plugin-controller"]

FROM runtime AS core
COPY --from=console-build /src/apps/console/dist /opt/webdock/ui
ENV WEBDOCK_BIND_ADDRESS=0.0.0.0 PORT=3130 WEBDOCK_UI_DIST=/opt/webdock/ui
EXPOSE 3130
ENTRYPOINT ["/usr/local/bin/webdock-control-plane"]
