# Runtime is pinned; the release pipeline must also record the resolved image digest.
FROM oven/bun:1.4.3 AS runtime
WORKDIR /app
# Build context must be prepared with bun install --frozen-lockfile + bun run --filter @webdock/api build.
# Copy only the API artifact, installed dependencies and workspace packages.
COPY --chown=bun:bun node_modules ./node_modules
COPY --chown=bun:bun packages ./packages
COPY --chown=bun:bun modules ./modules
COPY --chown=bun:bun adapters ./adapters
COPY --chown=bun:bun integrations ./integrations
COPY --chown=bun:bun apps/control-plane/node_modules ./apps/control-plane/node_modules
COPY --chown=bun:bun apps/control-plane/package.json ./apps/control-plane/package.json
COPY --chown=bun:bun apps/control-plane/dist ./apps/control-plane/dist
ENV NODE_ENV=production BIND_ADDRESS=0.0.0.0 PORT=3130
USER bun
WORKDIR /app/apps/control-plane
EXPOSE 3130
CMD ["bun", "dist/index.ts"]
