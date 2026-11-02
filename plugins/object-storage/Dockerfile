# Build from the repository root: docker build -f plugins/object-storage/Dockerfile .
# Official multi-platform digests verified on 2026-10-25. No root lockfile is consumed.
FROM scratch AS source
COPY . /

FROM docker.io/library/rust:1.94.0-slim-trixie@sha256:f7bf1c266d9e48c8d724733fd97ba60464c44b743eb4f46f935577d3242d81d0 AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY --from=source / /src
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/plugins/object-storage/target \
    cargo build --manifest-path plugins/object-storage/Cargo.toml --locked --release -j 2 \
    && cp plugins/object-storage/target/release/webdock-object-storage-plugin /webdock-object-storage-plugin

FROM docker.io/library/debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && test -s /etc/ssl/certs/ca-certificates.crt
COPY --from=build /webdock-object-storage-plugin /usr/local/bin/webdock-object-storage-plugin
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/webdock-object-storage-plugin"]
