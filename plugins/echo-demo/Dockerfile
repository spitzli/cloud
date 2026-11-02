# Build from the repository root: docker build -f plugins/echo-demo/Dockerfile .
FROM docker.io/library/rust:1.98.1-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/plugins/echo-demo/target \
    cargo build --manifest-path plugins/echo-demo/Cargo.toml --locked --release \
    && cp plugins/echo-demo/target/release/webdock-echo-demo /webdock-echo-demo

FROM docker.io/library/debian:trixie-slim
COPY --from=build /webdock-echo-demo /usr/local/bin/webdock-echo-demo
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/webdock-echo-demo"]
