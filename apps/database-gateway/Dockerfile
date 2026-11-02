# Compatibility build path; repository-root context, mirrored from deploy/docker.
# Repository-root context. The database browser remains the existing verified Rust runtime.
ARG TABULARIS_IMAGE=webdock-tabularis:production-fdd6c0a6
FROM rust:1.98.1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/app/target \
    cargo build --release --locked -p webdock-database-gateway && \
    cp target/release/webdock-database-gateway /usr/local/bin/webdock-database-gateway

FROM ${TABULARIS_IMAGE} AS database-gateway
COPY --from=build /usr/local/bin/webdock-database-gateway /usr/local/bin/webdock-database-gateway
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
ARG TABULARIS_SOURCE_COMMIT=fdd6c0a6f979ede0acb38415fa12b25c04aa9c4e
LABEL org.opencontainers.image.source="https://github.com/TabularisDB/tabularis" webdock.dev.tabularis-source=$TABULARIS_SOURCE_COMMIT webdock.dev.backend="rust"
USER 65532:65532
ENV NODE_ENV=production HOST=0.0.0.0 PORT=3137 TABULARIS_EXPECTED_COMMIT=$TABULARIS_SOURCE_COMMIT
EXPOSE 3137
ENTRYPOINT ["/usr/local/bin/webdock-database-gateway"]
