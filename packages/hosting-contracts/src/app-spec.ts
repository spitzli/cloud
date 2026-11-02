import { z } from "zod";
const healthHost = z.string().max(259).regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*(?::[1-9][0-9]{0,4})?$/).refine(value => {
  const [host = "", port] = value.split(":");
  try { return host.length <= 253 && (!port || Number(port) <= 65535) && new URL(`https://${value}`).hostname === host; }
  catch { return false; }
}).optional();
export const healthcheckImage =
  "docker.io/rancher/mirrored-library-traefik@sha256:f86a2cab1b5c649070c49f883c743dd32d8485a56e3368c5f93b9e91f1e91259";
export const appSpecSchema = z
  .object({
    template: z.enum(["healthcheck", "custom"]),
    image: z
      .string()
      .max(512)
      .regex(
        /^[a-zA-Z0-9.-]+(?::[0-9]+)?\/[a-zA-Z0-9_./-]+@sha256:[a-f0-9]{64}$/,
      ),
    args: z.array(z.string().max(512)).max(20),
    port: z.number().int().min(1024).max(65535),
    healthPath: z
      .string()
      .max(160)
      .regex(/^\/[a-zA-Z0-9/_-]*$/),
    healthHost,
    cpuMillicores: z.number().int().min(10).max(64000),
    memoryBytes: z.number().int().min(16777216).max(1099511627776),
    ephemeralBytes: z.number().int().min(1048576).max(1099511627776),
    replicas: z.number().int().min(0).max(20),
    volumeBytes: z
      .number()
      .int()
      .min(0)
      .max(1099511627776)
      .refine((v) => v === 0 || v >= 67108864)
      .optional(),
  })
  .strict()
  .refine(
    (s) => !s.volumeBytes || s.replicas <= 1,
    "Persistent storage supports one instance.",
  );
export type AppSpec = z.infer<typeof appSpecSchema>;
export const preparedAppSpecSchema = z
  .object({
    args: appSpecSchema.shape.args,
    port: appSpecSchema.shape.port,
    healthPath: appSpecSchema.shape.healthPath,
    healthHost,
    cpuMillicores: appSpecSchema.shape.cpuMillicores,
    memoryBytes: appSpecSchema.shape.memoryBytes,
    ephemeralBytes: appSpecSchema.shape.ephemeralBytes,
    replicas: appSpecSchema.shape.replicas,
    volumeBytes: appSpecSchema.shape.volumeBytes,
  })
  .strict()
  .refine(
    (s) => !s.volumeBytes || s.replicas <= 1,
    "Persistent storage supports one instance.",
  );
export type PreparedAppSpec = z.infer<typeof preparedAppSpecSchema>;

export function resolveAppSpec(value: unknown): AppSpec {
  const spec = appSpecSchema.parse(value);
  return spec.template === "healthcheck"
    ? {
        ...spec,
        image: healthcheckImage,
        args: [
          "--entrypoints.web.address=:8080",
          "--ping",
          "--ping.entrypoint=web",
        ],
        port: 8080,
        healthPath: "/ping",
      }
    : spec;
}
