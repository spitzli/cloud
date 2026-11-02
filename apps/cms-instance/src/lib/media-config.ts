import { isIP } from "node:net";

export interface LocalMediaStorage {
  kind: "local";
}
export interface S3MediaStorage {
  kind: "s3";
  resource: string;
  endpoint: string;
  region: string;
  bucket: string;
  prefix: string;
  credentials: { accessKeyId: string; secretAccessKey: string } | null;
}
export type MediaStorage = LocalMediaStorage | S3MediaStorage;
export function loadMediaStorage(
  env: Record<string, string | undefined>,
  identity: {
    instance: string;
    project: string;
    environment: string | null;
    migrating: boolean;
  },
): MediaStorage {
  const kind = env.CMS_MEDIA_STORAGE ?? "local";
  if (kind === "local") {
    if (
      Object.keys(env).some(
        (name) => name.startsWith("CMS_MEDIA_S3_") && env[name],
      )
    )
      throw Error("S3 media requires explicit CMS_MEDIA_STORAGE=s3");
    return { kind: "local" };
  }
  if (kind !== "s3") throw Error("Unsupported CMS media storage");
  const required = (name: string) => {
    const value = env[name];
    if (!value || value.length > 4096 || /[\x00-\x1f\x7f]/.test(value))
      throw Error(`Missing or invalid ${name}`);
    return value;
  };
  for (const part of [
    identity.instance,
    identity.project,
    identity.environment,
  ])
    if (!part || !/^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/.test(part))
      throw Error(
        "S3 media requires an explicit instance, project and environment",
      );
  const resource = required("CMS_MEDIA_S3_RESOURCE_ID");
  if (
    !/^[a-f0-9]{8}-[a-f0-9]{4}-[1-8][a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(
      resource,
    )
  )
    throw Error("Invalid CMS media resource");
  const endpoint = new URL(required("CMS_MEDIA_S3_ENDPOINT"));
  const hostname = endpoint.hostname.replace(/^\[|\]$/g, "");
  const loopback =
    isIP(hostname) !== 0 && (hostname.startsWith("127.") || hostname === "::1");
  if (
    (endpoint.protocol !== "https:" &&
      !(
        endpoint.protocol === "http:" &&
        loopback &&
        env.CMS_MEDIA_S3_LOOPBACK_TEST === "true"
      )) ||
    endpoint.pathname !== "/" ||
    endpoint.search ||
    endpoint.hash ||
    endpoint.username ||
    endpoint.password
  )
    throw Error("CMS media requires an exact verified HTTPS storage origin");
  const region = required("CMS_MEDIA_S3_REGION"),
    bucket = required("CMS_MEDIA_S3_BUCKET");
  if (
    !/^[a-z0-9-]{1,64}$/.test(region) ||
    !/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(bucket)
  )
    throw Error("Invalid CMS media bucket or region");
  if (
    identity.migrating &&
    (env.CMS_MEDIA_S3_ACCESS_KEY_ID || env.CMS_MEDIA_S3_SECRET_ACCESS_KEY)
  )
    throw Error("Media data credentials must not be present in migration jobs");
  return {
    kind: "s3",
    resource,
    endpoint: endpoint.origin,
    region,
    bucket,
    prefix: `cms/${identity.project}/${identity.environment}/${identity.instance}/media`,
    credentials: identity.migrating
      ? null
      : {
          accessKeyId: required("CMS_MEDIA_S3_ACCESS_KEY_ID"),
          secretAccessKey: required("CMS_MEDIA_S3_SECRET_ACCESS_KEY"),
        },
  };
}
