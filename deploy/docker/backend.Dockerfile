# Platform backend only. Frontend assets are built and hosted separately on Vercel.
FROM scratch AS source-context
COPY . /src

FROM docker.io/library/rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS build-base
RUN test -s /etc/ssl/certs/ca-certificates.crt \
 && sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt -o APT::Update::Error-Mode=any update \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt install -y --no-install-recommends protobuf-compiler pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
FROM build-base AS rust-build
WORKDIR /src
COPY --from=source-context /src .
ARG CARGO_BUILD_JOBS=2
# Quiet progress keeps the same bounded build-log policy used for customer builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --locked --release --quiet --jobs "$CARGO_BUILD_JOBS" -p webdock-control-plane --bins \
 && cargo build --locked --release --quiet --jobs "$CARGO_BUILD_JOBS" -p webdock-build --features worker,kubernetes-job --bin webdock-kubernetes-build-worker \
 && mkdir /binaries \
 && cp target/release/webdock-control-plane target/release/webdock-migrate \
       target/release/webdock-bootstrap-operator target/release/webdock-agent-control \
       target/release/webdock-import-clerk-waitlist \
       target/release/webdock-plugin-control target/release/webdock-plugin-controller \
       target/release/webdock-kubernetes-build-worker /binaries/

FROM docker.io/library/debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS runtime
# Bootstrap verified HTTPS APT with public CA roots, never disabled certificate checks.
COPY --from=build-base /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt -o APT::Update::Error-Mode=any update \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt install -y --no-install-recommends ca-certificates libssl3t64 libzstd1 libbrotli1 zlib1g \
 && rm -rf /var/lib/apt/lists/*
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
COPY vendor/maylily/LICENSE /usr/share/licenses/webdock/maylily.LICENSE
COPY --from=rust-build /binaries/ /usr/local/bin/
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/spitzli/webdock" \
      org.opencontainers.image.revision=$SOURCE_REVISION
USER 65532:65532
ENV NODE_ENV=production
WORKDIR /opt/webdock

FROM runtime AS migrate
ENTRYPOINT ["/usr/local/bin/webdock-migrate"]

FROM runtime AS agent-control
ENTRYPOINT ["/usr/local/bin/webdock-agent-control"]

FROM runtime AS plugin-control
ENTRYPOINT ["/usr/local/bin/webdock-plugin-control"]

FROM runtime AS plugin-controller
ENTRYPOINT ["/usr/local/bin/webdock-plugin-controller"]

FROM runtime AS build-controller
ENTRYPOINT ["/usr/local/bin/webdock-kubernetes-build-worker"]

FROM runtime AS backend
ENV WEBDOCK_BIND_ADDRESS=0.0.0.0 PORT=3130
EXPOSE 3130
ENTRYPOINT ["/usr/local/bin/webdock-control-plane"]
