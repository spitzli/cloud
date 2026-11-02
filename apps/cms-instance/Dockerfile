# Build from the repository root. Payload is deliberately pinned to the installed canary.
FROM oven/bun:1.4.3 AS build
WORKDIR /app
COPY package.json bun.lock bunfig.toml ./
COPY patches ./patches
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/webdock/
COPY apps ./apps
COPY packages ./packages
COPY modules ./modules
COPY adapters ./adapters
COPY integrations ./integrations
COPY vendor ./vendor
RUN bun install --frozen-lockfile --ignore-scripts
# The npm "bun" package (pinned by the control plane) needs its skipped postinstall; this image already ships that Bun.
RUN rm -f node_modules/.bin/bun node_modules/.bin/bunx
RUN bun run --filter @webdock/cms-instance build
# Runtime only applies reviewed SQL; migration generation and TS loaders are excluded.
RUN rm -rf /app/apps/cms-instance/.next/standalone/node_modules/drizzle-kit /app/apps/cms-instance/.next/standalone/node_modules/esbuild /app/apps/cms-instance/.next/standalone/node_modules/@esbuild /app/apps/cms-instance/.next/standalone/node_modules/payload/node_modules/esbuild /app/apps/cms-instance/.next/standalone/node_modules/payload/node_modules/@esbuild /app/apps/cms-instance/.next/standalone/node_modules/payload/node_modules/tsx

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288 AS runtime
ENV NODE_ENV=production PORT=3000 HOSTNAME=0.0.0.0 NEXT_TELEMETRY_DISABLED=1
WORKDIR /app
# Standalone carries only the CMS server and traced production dependencies.
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/standalone ./
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/static ./apps/cms-instance/.next/static
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/migrate.mjs ./migrate.mjs
RUN mkdir -p /data/media /data/transfers && chown -R 65532:65532 /data
COPY --from=build /usr/share/licenses/webdock/ /usr/share/licenses/webdock/
USER 65532:65532
VOLUME ["/data"]
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s CMD ["bun","-e","fetch('http://127.0.0.1:3000/api/health').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"]
CMD ["bun","apps/cms-instance/server.js"]
