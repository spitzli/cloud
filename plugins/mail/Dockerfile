# Build from the repository root: docker build -f plugins/mail/Dockerfile .
FROM docker.io/library/rust:1.98.1-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/plugins/mail/target \
    cargo build --manifest-path plugins/mail/Cargo.toml --locked --release \
    && cp plugins/mail/target/release/spitzli-mail /spitzli-mail \
    && cp plugins/mail/target/release/spitzli-mail-gateway /spitzli-mail-gateway

FROM docker.io/library/debian:trixie-slim
COPY --from=build /spitzli-mail /usr/local/bin/spitzli-mail
COPY --from=build /spitzli-mail-gateway /usr/local/bin/spitzli-mail-gateway
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/spitzli-mail"]
