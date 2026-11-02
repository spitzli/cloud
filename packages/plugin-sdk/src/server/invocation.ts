import { createPublicKey, verify } from "node:crypto";
import { z } from "zod";
import { PluginError } from "../contracts/errors.js";
export const invocationClaimsSchema = z.strictObject({
  v: z.literal(1),
  id: z.uuid(),
  pluginID: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
  generation: z.number().int().positive().safe(),
  action: z.string().min(1).max(64),
  projectID: z.string().regex(/^[0-9]{1,20}$/),
  customerID: z.string().regex(/^[1-9][0-9]{0,18}$/),
  iat: z.number().int(),
  exp: z.number().int(),
});
export type InvocationClaims = z.infer<typeof invocationClaimsSchema>;
/** Signature proves the host issued this request; the host rechecks live authority on every service call. */
export function verifyInvocation(
  token: string,
  options: {
    publicKey: string;
    pluginID: string;
    generation: number;
    now?: number;
  },
): InvocationClaims {
  try {
    if (token.length > 16384 || !/^[-_A-Za-z0-9]+\.[-_A-Za-z0-9]+$/.test(token))
      throw Error();
    const [body, signature] = token.split(".");
    const key = createPublicKey(options.publicKey);
    if (
      key.asymmetricKeyType !== "ed25519" ||
      !verify(null, Buffer.from(body), key, Buffer.from(signature, "base64url"))
    )
      throw Error();
    const claims = invocationClaimsSchema.parse(
      JSON.parse(Buffer.from(body, "base64url").toString("utf8")),
    );
    const now = options.now ?? Math.floor(Date.now() / 1000);
    if (
      claims.pluginID !== options.pluginID ||
      claims.generation !== options.generation ||
      claims.iat > now ||
      claims.exp <= now ||
      claims.exp <= claims.iat ||
      claims.exp - claims.iat > 60
    )
      throw Error();
    return claims;
  } catch {
    throw new PluginError("permission_denied", "Invalid plugin invocation");
  }
}
