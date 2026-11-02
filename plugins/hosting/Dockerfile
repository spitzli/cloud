# Build from the repository root: docker build -f plugins/hosting/Dockerfile .
FROM docker.io/library/rust:1.98.1-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/plugins/hosting/target \
    cargo build --manifest-path plugins/hosting/Cargo.toml --locked --release \
    && cp plugins/hosting/target/release/webdock-hosting /webdock-hosting

FROM docker.io/library/debian:trixie-slim
COPY --from=build /webdock-hosting /usr/local/bin/webdock-hosting
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/webdock-hosting"]
