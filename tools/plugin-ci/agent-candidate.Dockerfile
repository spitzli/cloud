# Trusted, frozen platform source only. Produces private files, never an image push.
FROM docker.io/library/rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730 AS build
ENV CARGO_BUILD_JOBS=3 CARGO_TARGET_DIR=/agent-target
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="/usr/sbin/runuser -u nobody --"
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config libssl-dev binutils python3 python3-yaml ca-certificates curl util-linux && rm -rf /var/lib/apt/lists/* \
    && rustup component add --toolchain 1.98.1-x86_64-unknown-linux-gnu rustfmt clippy
RUN curl --fail --silent --show-error --location https://get.helm.sh/helm-v3.21.0-linux-amd64.tar.gz --output /tmp/helm.tar.gz \
    && echo '0093eb572e3d2380f094df162ddb525e219249de88957afe24cfbb19632acd36  /tmp/helm.tar.gz' | sha256sum --check - \
    && tar -xzf /tmp/helm.tar.gz -C /tmp linux-amd64/helm && install -m 0555 /tmp/linux-amd64/helm /usr/local/bin/helm \
    && curl --fail --silent --show-error --location https://dl.k8s.io/release/v1.37.2/bin/linux/amd64/kubectl --output /tmp/kubectl \
    && echo 'c04487523e7aa7d1493f7808174c899db93d7a9cf57531420cd76b9bd415f8f4  /tmp/kubectl' | sha256sum --check - \
    && install -m 0555 /tmp/kubectl /usr/local/bin/kubectl && rm -rf /tmp/helm.tar.gz /tmp/linux-amd64 /tmp/kubectl
WORKDIR /src
COPY source.tar /checkpoint/source.tar
COPY checkpoint.json /checkpoint/checkpoint.json
COPY agent_build.py agent_elf.py /harness/
ARG SOURCE_SHA256
ARG BUILD_MODE=preflight
RUN python3 /harness/agent_build.py unpack --archive /checkpoint/source.tar --source-sha256 "$SOURCE_SHA256" --output /src
RUN --mount=type=cache,id=spitzli-agent-bookworm-dacc9e51f252-1-98-1-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=spitzli-agent-bookworm-dacc9e51f252-1-98-1-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=spitzli-agent-bookworm-dacc9e51f252-1-98-1-kubernetes-target,target=/agent-target,sharing=locked \
    set -eu; mkdir /out; \
    run_checked() { logfile="$1"; shift; if "$@" > "$logfile" 2>&1; then return 0; else tail -c 2097152 "$logfile" >&2; exit 1; fi; }; \
    if [ "$BUILD_MODE" = preflight ]; then \
      run_checked /out/agent-lib-tests.log cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --lib --jobs 3 -- --test-threads=3 --format pretty; \
    elif [ "$BUILD_MODE" = fixture-refresh ] || [ "$BUILD_MODE" = fixture-only ]; then \
      cp /checkpoint/checkpoint.json /out/checkpoint.json; \
      run_checked /out/integration-artifacts.json cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --test legacy_attachment --no-run --message-format=json --jobs 3; \
      python3 /harness/agent_build.py export-integration --messages /out/integration-artifacts.json --output /out > /out/integration-build.json; \
    else \
      test "$BUILD_MODE" = final; \
    run_checked /out/storage-attachment-tests.log cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --lib --jobs 3 -- --test-threads=3 --format pretty; \
    run_checked /out/management-tests.log cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --test management --jobs 3 -- --test-threads=3 --format pretty; \
    run_checked /out/full-agent-tests.log cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --all-targets --jobs 3 -- --test-threads=3 --format pretty; \
    run_checked /out/integration-artifacts.json cargo test --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --test legacy_attachment --no-run --message-format=json --jobs 3; \
    python3 /harness/agent_build.py export-integration --messages /out/integration-artifacts.json --output /out > /out/integration-build.json; \
    fi; \
    python3 /harness/agent_build.py validate-tests --mode "$BUILD_MODE" --output /out > /out/test-counts.json; \
    if [ "$BUILD_MODE" != fixture-only ]; then \
    run_checked /out/release-build.log cargo build --target x86_64-unknown-linux-gnu --locked -p webdock-agent --no-default-features --features kubernetes --bin webdock-agent --release --jobs 3; \
    install -m 0555 /agent-target/x86_64-unknown-linux-gnu/release/webdock-agent /out/webdock-agent; \
    python3 /harness/agent_elf.py /out/webdock-agent --output /out/bookworm-elf-proof.json; \
    (cd /out; sha256sum webdock-agent > webdock-agent.sha256); \
    fi; \
    rustc --version --verbose > /out/rustc-version.txt; \
    helm version --short > /out/helm-version.txt; \
    kubectl version --client --output=json > /out/kubectl-version.json; \
    getconf GNU_LIBC_VERSION > /out/builder-libc.txt; \
    dpkg-query -W > /out/builder-packages.txt; \
    cp /checkpoint/checkpoint.json /out/checkpoint.json
FROM scratch AS artifact
COPY --from=build /out/ /
