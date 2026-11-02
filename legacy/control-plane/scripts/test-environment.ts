import { randomBytes } from "node:crypto";
import { assertTestDatabase } from "../tests/helpers/database";

export function testEnvironment(
  source: Partial<NodeJS.ProcessEnv>,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { NODE_ENV: "development" };
  for (const name of ["PATH", "HOME", "TMPDIR", "TERM", "NO_COLOR"])
    if (source[name]) env[name] = source[name];
  Object.assign(env, {
    NODE_ENV: "development",
    AUTH_TEST_MAIL: "true",
    WEBDOCK_MODULES: JSON.stringify({configVersion: 1, modules: Object.fromEntries(["mail", "hosting", "databases", "storage"].map(id => [id, {enabled: true}]))}),
    BETTER_AUTH_URL: "http://localhost:3125",
    BETTER_AUTH_SECRET: randomBytes(32).toString("hex"),
    PAYLOAD_SECRET: randomBytes(32).toString("hex"),
  });
  if (source.API_TEST_DATABASE_URL) {
    assertTestDatabase(source.API_TEST_DATABASE_URL);
    for (const key of ["BETTER_AUTH_SECRET", "PAYLOAD_SECRET"])
      if (source[key]) env[key] = source[key];
    Object.assign(env, {
      API_TEST_DATABASE_URL: source.API_TEST_DATABASE_URL,
      DATABASE_URL: source.API_TEST_DATABASE_URL,
      DATABASE_URL_UNPOOLED: source.API_TEST_DATABASE_URL,
    });
  }
  if (source.WEBDOCK_JOBS_REDIS_URL) {
    const redis = new URL(source.WEBDOCK_JOBS_REDIS_URL);
    if (redis.protocol !== "redis:" || !["127.0.0.1", "localhost"].includes(redis.hostname) || redis.search || redis.hash)
      throw Error("Use a loopback Redis test fixture");
    env.WEBDOCK_JOBS_REDIS_URL = redis.href;
  }
  return env;
}

export function rlsEnvironment(
  source: Partial<NodeJS.ProcessEnv>,
  fixture: unknown,
  ownerURL: string,
): NodeJS.ProcessEnv {
  const validate = (raw: string) => {
    const url = new URL(raw);
    if (
      !["postgres:", "postgresql:"].includes(url.protocol) ||
      !["127.0.0.1", "localhost"].includes(url.hostname) ||
      url.pathname !== "/webdock_rls_test" ||
      url.search ||
      url.hash
    )
      throw Error("Use the isolated loopback RLS fixture");
    return url;
  };
  const owner = validate(ownerURL);
  if (
    !fixture ||
    typeof fixture !== "object" ||
    !("roles" in fixture) ||
    !("contextSecret" in fixture) ||
    typeof fixture.contextSecret !== "string" ||
    fixture.contextSecret.length < 32 ||
    !fixture.roles ||
    typeof fixture.roles !== "object"
  )
    throw Error("Invalid restricted-role fixture");
  const roles = fixture.roles as Record<string, unknown>;
  // Validate every role, including optional browser roles used by ported tests.
  for (const [role, raw] of Object.entries(roles)) {
    if (typeof raw !== "string") throw Error("Invalid fixture role URL");
    const url = validate(raw);
    if (
      url.host !== owner.host ||
      url.pathname !== owner.pathname ||
      decodeURIComponent(url.username) !== role
    )
      throw Error(
        "Fixture roles must use their named principal on the owner endpoint",
      );
  }
  const env = testEnvironment({ ...source, API_TEST_DATABASE_URL: undefined });
  if (!source.BETTER_AUTH_SECRET || source.BETTER_AUTH_SECRET.length < 32)
    throw Error("Explicit stable RLS fixture auth key required");
  env.BETTER_AUTH_SECRET = source.BETTER_AUTH_SECRET;
  for (const [key, role] of Object.entries({
    DATABASE_URL: "webdock_auth_runtime",
    AUTH_DATABASE_URL: "webdock_auth_bootstrap",
    DATABASE_WORKER_URL: "webdock_worker_runtime",
    REGISTRY_DATABASE_URL: "webdock_registry_runtime",
  })) {
    if (typeof roles[role] !== "string")
      throw Error("Missing restricted test role: " + role);
    env[key] = roles[role] as string;
  }
  Object.assign(env, {
    API_TEST_OWNER_URL: ownerURL,
    DATABASE_RLS_SECRET: fixture.contextSecret,
    WEBDOCK_RLS_FIXTURE_FILE: source.WEBDOCK_RLS_FIXTURE_FILE,
    WEBDOCK_API_DOMAINS: "auth,registry",
  });
  return env;
}
