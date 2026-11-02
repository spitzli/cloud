# The official SDK is bundled locally; this image contains no database or Neon credentials.
FROM docker.io/oven/bun@sha256:c017a211c49a21adc0e49dda7b60ac8112887c3d9465e01427f434ed107c830e
WORKDIR /opt/webdock-live
COPY apps/realtime-sealer/dist/ ./
USER 65532:65532
ENV NODE_ENV=production
ENTRYPOINT ["bun", "--no-env-file", "server.js"]
