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
    cargo build --locked --release --quiet --jobs "$CARGO_BUILD_JOBS" -p spitzli-control-plane --bins \
 && cargo build --locked --release --quiet --jobs "$CARGO_BUILD_JOBS" -p spitzli-build --features worker,kubernetes-job --bin spitzli-kubernetes-build-worker \
 && mkdir /binaries \
 && cp target/release/spitzli-control-plane target/release/spitzli-migrate \
       target/release/spitzli-bootstrap-operator target/release/spitzli-agent-control \
       target/release/spitzli-import-clerk-waitlist \
       target/release/spitzli-plugin-control target/release/spitzli-plugin-controller \
       target/release/spitzli-kubernetes-build-worker /binaries/

FROM docker.io/library/debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS runtime
# Bootstrap verified HTTPS APT with public CA roots, never disabled certificate checks.
COPY --from=build-base /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt -o APT::Update::Error-Mode=any update \
 && apt-get -qq -o Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt install -y --no-install-recommends ca-certificates libssl3t64 libzstd1 libbrotli1 zlib1g \
 && rm -rf /var/lib/apt/lists/*
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/
COPY vendor/maylily/LICENSE /usr/share/licenses/spitzli-cloud/maylily.LICENSE
COPY --from=rust-build /binaries/ /usr/local/bin/
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/spitzli/cloud" \
      org.opencontainers.image.revision=$SOURCE_REVISION \
      org.opencontainers.image.title="Spitzli Cloud" \
      org.opencontainers.image.licenses="Apache-2.0"
USER 65532:65532
ENV NODE_ENV=production
WORKDIR /opt/webdock

FROM runtime AS migrate
ENTRYPOINT ["/usr/local/bin/spitzli-migrate"]

FROM runtime AS agent-control
ENTRYPOINT ["/usr/local/bin/spitzli-agent-control"]

FROM runtime AS plugin-control
ENTRYPOINT ["/usr/local/bin/spitzli-plugin-control"]

FROM runtime AS plugin-controller
ENTRYPOINT ["/usr/local/bin/spitzli-plugin-controller"]

FROM runtime AS build-controller
ENTRYPOINT ["/usr/local/bin/spitzli-kubernetes-build-worker"]

FROM runtime AS backend
ENV WEBDOCK_BIND_ADDRESS=0.0.0.0 PORT=3130
EXPOSE 3130
ENTRYPOINT ["/usr/local/bin/spitzli-control-plane"]
