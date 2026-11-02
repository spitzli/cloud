import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose";
import { z } from "zod";
import { sourceFail, sourceLimit } from "./sources-policy";
const id = z.string().regex(/^[1-9][0-9]{0,19}$/);
const sha = z.string().regex(/^[a-f0-9]{40}$/);
export const actionsSourceInput = z.strictObject({
  id,
  nonce: z.string().regex(/^[a-f0-9]{32}$/),
  offset: z.number().int().min(0).max(sourceLimit),
});
export const actionsMetadataSchema = z
  .strictObject({
    repositoryID: id,
    repository: z
      .string()
      .max(201)
      .regex(/^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/),
    workflowRef: z
      .string()
      .max(600)
      .regex(
        /^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+\/\.github\/workflows\/[A-Za-z0-9_.-]+\.ya?ml@refs\/(heads|tags)\/[A-Za-z0-9_./-]+$/,
      ),
    workflowSHA: sha,
    nonce: z.string().regex(/^[a-f0-9]{32}$/),
    size: z.number().int().positive().max(sourceLimit),
    sourceSHA256: z.string().regex(/^[a-f0-9]{64}$/),
    commit: sha.optional(),
    runID: id.optional(),
  })
  .refine(
    (value) =>
      value.workflowRef.startsWith(value.repository + "/.github/workflows/") &&
      !value.workflowRef.includes(".."),
  );
export const actionsPrepareSchema = z.strictObject({
  id,
  claimToken: z.uuid(),
  size: z.number().int().positive().max(sourceLimit),
  sourceSHA256: z.string().regex(/^[a-f0-9]{64}$/),
  commit: sha.optional(),
  repositoryID: id,
  repository: z.string().max(201),
  workflowRef: z.string().max(600),
  workflowSHA: sha,
  nonce: z.string().regex(/^[a-f0-9]{32}$/),
});
export const actionsChunkSchema = z.strictObject({
  id,
  claimToken: z.uuid(),
  offset: z.number().int().min(0).max(sourceLimit),
  data: z.string().min(1).max(87384),
});
export const actionsBindSchema = z.strictObject({
  id,
  claimToken: z.uuid(),
  runID: id,
});
const claimsSchema = z.object({
  repository_id: id,
  repository: z.string().max(201),
  repository_visibility: z.literal("private"),
  workflow_ref: z.string().max(600),
  workflow_sha: sha,
  run_id: id,
  run_attempt: z.literal("1"),
  event_name: z.literal("workflow_dispatch"),
});
export type ActionsClaims = z.infer<typeof claimsSchema>;
const issuer = "https://token.actions.githubusercontent.com";
const githubKeys = createRemoteJWKSet(new URL(issuer + "/.well-known/jwks"), {
  timeoutDuration: 5000,
  cooldownDuration: 30000,
});
export function createGitHubActionsVerifier(
  keys: JWTVerifyGetKey = githubKeys,
) {
  return async (
    authorization: string | null,
    input: z.infer<typeof actionsSourceInput>,
  ): Promise<ActionsClaims> => {
    try {
      if (
        !authorization ||
        authorization.length > 16384 ||
        !/^Bearer [A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(
          authorization,
        )
      )
        throw Error();
      const audience = `webdock-plugin-build:${input.id}:${input.nonce}`;
      const { payload } = await jwtVerify(authorization.slice(7), keys, {
        issuer,
        audience,
        algorithms: ["RS256"],
        requiredClaims: ["exp", "iat", "nbf"],
        maxTokenAge: "10 minutes",
        clockTolerance: 5,
      });
      if (payload.aud !== audience) throw Error();
      return claimsSchema.parse(payload);
    } catch {
      sourceFail(
        "actions_unauthorized",
        "A verified private GitHub Actions build identity is required.",
        401,
      );
    }
  };
}
export function requireActionsIdentity(
  metadata: unknown,
  claims: ActionsClaims,
  input: z.infer<typeof actionsSourceInput>,
) {
  const parsed = actionsMetadataSchema.safeParse(metadata),
    identity = claimsSchema.safeParse(claims);
  if (!parsed.success || !identity.success)
    sourceFail(
      "actions_unauthorized",
      "This Actions run cannot access the source import.",
      401,
    );
  const m = parsed.data,
    c = identity.data;
  if (
    input.nonce !== m.nonce ||
    c.repository_id !== m.repositoryID ||
    c.repository !== m.repository ||
    c.workflow_ref !== m.workflowRef ||
    c.workflow_sha !== m.workflowSHA
  )
    sourceFail(
      "actions_unauthorized",
      "This Actions run cannot access the source import.",
      401,
    );
  if (!m.runID)
    sourceFail(
      "actions_binding_pending",
      "Wait for the installer to bind this GitHub Actions run, then retry.",
      503,
    );
  if (c.run_id !== m.runID)
    sourceFail(
      "actions_unauthorized",
      "This Actions run cannot access the source import.",
      401,
    );
  return m;
}
