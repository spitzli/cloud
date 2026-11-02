FROM oven/bun:1.4.3 AS build
WORKDIR /app
COPY package.json bun.lock ./
COPY vendor ./vendor
RUN bun install --frozen-lockfile --ignore-scripts
COPY tsconfig.json ./
COPY src ./src
RUN bun run build && rm -rf node_modules && bun install --frozen-lockfile --ignore-scripts --production

FROM oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288
ENV NODE_ENV=production
WORKDIR /app
COPY --from=build /app/package.json ./
COPY --from=build /app/vendor ./vendor
COPY --from=build /app/node_modules ./node_modules
COPY --from=build /app/dist ./dist
USER bun
EXPOSE 3443
CMD ["bun", "dist/index.js"]
