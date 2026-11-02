# Build from the repository root. No .env files or provider credentials are build inputs.
FROM oven/bun:1.4.3 AS dependencies
WORKDIR /app
COPY . .
RUN bun install --frozen-lockfile

FROM dependencies AS api-build
RUN bun run --filter @webdock/api build

FROM oven/bun:1.4.3 AS api
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
# FSR and workspace exports load source at runtime; preserve workspace paths.
COPY --from=api-build --chown=bun:bun /app /app
ENV NODE_ENV=production BIND_ADDRESS=0.0.0.0 PORT=3130
USER bun
WORKDIR /app/apps/control-plane
EXPOSE 3130
CMD ["bun", "dist/index.ts"]

FROM api AS worker
CMD ["bun", "dist/worker.ts"]

FROM api AS migrate
WORKDIR /app
ENTRYPOINT ["bun", "tools/migrations/install.ts"]
CMD []

FROM dependencies AS console-build
ENV NEXT_TELEMETRY_DISABLED=1
RUN bun run --filter @webdock/admin build

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS console
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
ENV NODE_ENV=production HOSTNAME=0.0.0.0 PORT=3120 NEXT_TELEMETRY_DISABLED=1
COPY --from=console-build --chown=bun:bun /app/apps/console/.next/standalone ./
COPY --from=console-build --chown=bun:bun /app/apps/console/.next/static ./apps/console/.next/static
COPY --from=console-build --chown=bun:bun /app/apps/console/public ./apps/console/public
USER bun
EXPOSE 3120
CMD ["bun", "apps/console/server.js"]

FROM dependencies AS auth-build
ENV NEXT_TELEMETRY_DISABLED=1
RUN bun run --filter @webdock/auth build

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS auth
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
ENV NODE_ENV=production HOSTNAME=0.0.0.0 PORT=3125 NEXT_TELEMETRY_DISABLED=1
COPY --from=auth-build --chown=bun:bun /app/apps/auth/.next/standalone ./
COPY --from=auth-build --chown=bun:bun /app/apps/auth/.next/static ./apps/auth/.next/static
COPY --from=auth-build --chown=bun:bun /app/apps/auth/public ./apps/auth/public
USER bun
EXPOSE 3125
CMD ["bun", "apps/auth/server.js"]

# Core distribution: no product packages or source tree in the runtime image.
FROM dependencies AS core-api-build
RUN bun run --filter @webdock/api build:core

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS api-core
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
COPY --from=core-api-build --chown=bun:bun /app/apps/control-plane/dist-core/core.js ./core.js
ENV NODE_ENV=production BIND_ADDRESS=0.0.0.0 PORT=3130
USER bun
EXPOSE 3130
CMD ["bun", "core.js"]

FROM dependencies AS core-console-build
ENV NEXT_TELEMETRY_DISABLED=1
RUN bun tools/release/build-core-console.mjs

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS console-core
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
ENV NODE_ENV=production HOSTNAME=0.0.0.0 PORT=3120 NEXT_TELEMETRY_DISABLED=1
COPY --from=core-console-build --chown=bun:bun /app/.core-build/console/.next/standalone ./
COPY --from=core-console-build --chown=bun:bun /app/.core-build/console/.next/static ./.core-build/console/.next/static
COPY --from=core-console-build --chown=bun:bun /app/.core-build/console/public ./.core-build/console/public
USER bun
EXPOSE 3120
CMD ["bun", ".core-build/console/server.js"]
