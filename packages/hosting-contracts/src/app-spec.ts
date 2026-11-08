import { z } from "zod";
const healthHost = z.string().max(259).regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*(?::[1-9][0-9]{0,4})?$/).refine(value => {
  const [host = "", port] = value.split(":");
  try { return host.length <= 253 && (!port || Number(port) <= 65535) && new URL(`https://${value}`).hostname === host; }
  catch { return false; }
}).optional();
export const healthcheckImage =
  "docker.io/rancher/mirrored-library-traefik@sha256:f86a2cab1b5c649070c49f883c743dd32d8485a56e3368c5f93b9e91f1e91259";
export const autoscalingPolicySchema = z.object({
  minReplicas: z.number().int().min(1).max(20),
  maxReplicas: z.number().int().min(1).max(20),
  targetCPUUtilizationPercent: z.number().int().min(10).max(90),
}).strict().refine(p => p.minReplicas <= p.maxReplicas, "Minimum instances must not exceed maximum instances.");
function validScaling(s: { replicas: number; volumeBytes?: number | undefined; autoscaling?: z.infer<typeof autoscalingPolicySchema> | undefined }) {
  return !s.autoscaling || (!s.volumeBytes && (s.replicas === 0 ||
    (s.replicas >= s.autoscaling.minReplicas && s.replicas <= s.autoscaling.maxReplicas)));
}
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
    autoscaling: autoscalingPolicySchema.optional(),
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
  )
  .refine(validScaling, "Automatic scaling requires no local volume and a stopped or in-range instance count.");
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
    autoscaling: autoscalingPolicySchema.optional(),
    volumeBytes: appSpecSchema.shape.volumeBytes,
  })
  .strict()
  .refine(
    (s) => !s.volumeBytes || s.replicas <= 1,
    "Persistent storage supports one instance.",
  )
  .refine(validScaling, "Automatic scaling requires no local volume and a stopped or in-range instance count.");
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
