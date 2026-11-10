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
    cargo build --locked --release -p spitzli-control-plane --bins \
    && mkdir /binaries \
    && cp target/release/spitzli-control-plane target/release/spitzli-migrate \
       target/release/spitzli-bootstrap-operator target/release/spitzli-agent-control \
       target/release/spitzli-plugin-control target/release/spitzli-plugin-controller /binaries/

FROM docker.io/library/debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3t64 libzstd1 libbrotli1 zlib1g && rm -rf /var/lib/apt/lists/*
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/
COPY --from=rust-build /binaries/ /usr/local/bin/
USER 65532:65532
ENV NODE_ENV=production
WORKDIR /opt/webdock

FROM runtime AS migrate
ENTRYPOINT ["/usr/local/bin/spitzli-migrate"]

FROM runtime AS bootstrap-operator
ENTRYPOINT ["/usr/local/bin/spitzli-bootstrap-operator"]

FROM runtime AS agent-control
ENTRYPOINT ["/usr/local/bin/spitzli-agent-control"]

FROM runtime AS plugin-control
ENTRYPOINT ["/usr/local/bin/spitzli-plugin-control"]

FROM runtime AS plugin-controller
ENTRYPOINT ["/usr/local/bin/spitzli-plugin-controller"]

FROM runtime AS core
COPY --from=console-build /src/apps/console/dist /opt/webdock/ui
ENV SPITZLI_BIND_ADDRESS=0.0.0.0 PORT=3130 SPITZLI_UI_DIST=/opt/webdock/ui
EXPOSE 3130
ENTRYPOINT ["/usr/local/bin/spitzli-control-plane"]
