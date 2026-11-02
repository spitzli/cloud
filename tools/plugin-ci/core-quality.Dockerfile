# Manual trusted-platform source checks only; no push, deployment or live credentials.
FROM docker.io/oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS bun
FROM docker.io/library/node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20 AS node
FROM docker.io/library/docker:29.9.2-cli@sha256:5e6baf67325a79f64ed3bed6863954d5e05d0f3d4e57cbdb14809346814bbfdb AS docker-cli
FROM docker.io/library/rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730 AS source
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config libssl-dev binutils git gettext python3 python3-yaml ca-certificates curl util-linux && rm -rf /var/lib/apt/lists/* \
 && rustup component add --toolchain 1.98.1-x86_64-unknown-linux-gnu rustfmt clippy \
 && useradd --uid 1001 --create-home --shell /bin/sh quality
RUN curl --fail --silent --show-error --location https://get.helm.sh/helm-v3.21.0-linux-amd64.tar.gz --output /tmp/helm.tar.gz \
 && echo '0093eb572e3d2380f094df162ddb525e219249de88957afe24cfbb19632acd36  /tmp/helm.tar.gz' | sha256sum --check - \
 && tar -xzf /tmp/helm.tar.gz -C /tmp linux-amd64/helm && install -m 0555 /tmp/linux-amd64/helm /usr/local/bin/helm \
 && curl --fail --silent --show-error --location https://dl.k8s.io/release/v1.37.2/bin/linux/amd64/kubectl --output /tmp/kubectl \
 && echo 'c04487523e7aa7d1493f7808174c899db93d7a9cf57531420cd76b9bd415f8f4  /tmp/kubectl' | sha256sum --check - \
 && install -m 0555 /tmp/kubectl /usr/local/bin/kubectl && rm -rf /tmp/helm.tar.gz /tmp/linux-amd64 /tmp/kubectl
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /usr/local/libexec/docker/cli-plugins/docker-compose /usr/local/libexec/docker/cli-plugins/docker-compose
ENV DOCKER_HOST=unix:///nonexistent/manual-quality-no-daemon.sock
ENV CARGO_BUILD_JOBS=3 RUST_TEST_THREADS=3 CARGO_HOME=/quality-cargo CARGO_TARGET_DIR=/quality-target HOME=/home/quality USER=quality LANG=C.UTF-8 CI=1 NODE_OPTIONS=--max-old-space-size=2048 PLAYWRIGHT_BROWSERS_PATH=/opt/playwright
WORKDIR /src
COPY source.tar checkpoint.json /checkpoint/
COPY core_quality.py agent_build.py core_tooling.py /harness/
ARG SOURCE_SHA256
RUN python3 /harness/core_quality.py unpack --archive /checkpoint/source.tar --source-sha256 "$SOURCE_SHA256" --output /src \
 && mkdir /results /opt/playwright && chown -R 1001:1001 /src /results /opt/playwright
USER 1001:1001
FROM source AS focused
RUN --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-registry,target=/quality-cargo,uid=1001,gid=1001,sharing=locked \
    --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-target,target=/quality-target,uid=1001,gid=1001,sharing=locked \
    python3 /harness/core_quality.py run --mode focused --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM source AS full
RUN --mount=type=cache,id=spitzli-core-quality-bun-1-4-3,target=/home/quality/.bun/install/cache,uid=1001,gid=1001,sharing=locked \
    bun install --frozen-lockfile --ignore-scripts
USER 0:0
RUN bun --bun node_modules/playwright/cli.js install-deps chromium
USER 1001:1001
RUN bun --bun node_modules/playwright/cli.js install chromium
RUN --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-registry,target=/quality-cargo,uid=1001,gid=1001,sharing=locked \
    --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-target,target=/quality-target,uid=1001,gid=1001,sharing=locked \
    python3 /harness/core_quality.py run --mode full --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM source AS consumer
RUN --mount=type=cache,id=spitzli-core-quality-bun-1-4-3,target=/home/quality/.bun/install/cache,uid=1001,gid=1001,sharing=locked \
    bun install --frozen-lockfile --ignore-scripts --filter @webdock/console
RUN python3 /harness/core_quality.py run --mode consumer --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM scratch AS consumer-artifact
COPY --from=consumer /results/out/ /
FROM scratch AS focused-artifact
COPY --from=focused /results/out/ /
FROM scratch AS full-artifact
COPY --from=full /results/out/ /

FROM docker.io/library/postgres:18-bookworm@sha256:5c4783a8cc3f0074912164f94dfa1be4e043438f0f707cf968d7af9a0d31a2c8 AS owned-database
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates libstdc++6 binutils && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1001 --create-home --shell /bin/sh quality && mkdir /results && chown 1001:1001 /results
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
COPY --from=source --chown=1001:1001 /src/ /src/
COPY --from=source /checkpoint/checkpoint.json /checkpoint/checkpoint.json
COPY core_local_fixtures.py website_fixture_seed.ts core_tooling.py /harness/
ENV HOME=/home/quality USER=quality LANG=C.UTF-8 CI=1 NODE_OPTIONS=--max-old-space-size=2048 NEXT_TELEMETRY_DISABLED=1
WORKDIR /src
USER 1001:1001
FROM owned-database AS migration
RUN python3 /harness/core_local_fixtures.py --mode migration --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM owned-database AS website
RUN --mount=type=cache,id=spitzli-core-quality-bun-1-4-3,target=/home/quality/.bun/install/cache,uid=1001,gid=1001,sharing=locked \
    bun install --frozen-lockfile --ignore-scripts --filter @webdock/web
RUN python3 /harness/core_local_fixtures.py --mode website --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM scratch AS migration-artifact
COPY --from=migration /results/out/ /
FROM scratch AS website-artifact
COPY --from=website /results/out/ /

FROM source AS native-pg-build
COPY core_native_pg.py /harness/
RUN --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-registry,target=/quality-cargo,uid=1001,gid=1001,sharing=locked \
    --mount=type=cache,id=spitzli-core-quality-bookworm-1-98-1-target,target=/quality-target,uid=1001,gid=1001,sharing=locked \
    python3 /harness/core_native_pg.py --phase compile --output /results/native-compile --checkpoint /checkpoint/checkpoint.json
FROM owned-database AS native-pg
COPY --from=native-pg-build --chown=1001:1001 /results/native-compile/ /results/native-compile/
COPY core_native_pg.py core_quality.py agent_build.py core_tooling.py /harness/
RUN python3 /harness/core_native_pg.py --phase runtime --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM scratch AS native-pg-artifact
COPY --from=native-pg /results/out/ /
FROM owned-database AS recovery
RUN --mount=type=cache,id=spitzli-core-quality-bun-1-4-3,target=/home/quality/.bun/install/cache,uid=1001,gid=1001,sharing=locked \
    bun install --frozen-lockfile --ignore-scripts
RUN bun --no-env-file -e "await import('pg'); await import('zod'); await import('./tools/migrations/neon-enrollment-recovery.ts')"
RUN python3 /harness/core_local_fixtures.py --mode recovery --output /results/out --checkpoint /checkpoint/checkpoint.json
FROM scratch AS recovery-artifact
COPY --from=recovery /results/out/ /
