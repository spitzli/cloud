import { compactVerify, importJWK, type JWK } from "jose";
import { createHash } from "node:crypto";
import {
  parsePluginManifest,
  type VerifiedPluginRelease,
} from "@spitzli/plugin-sdk/contracts";
const hash = (raw: string) => createHash("sha256").update(raw).digest("hex");
/** JSON.parse alone silently accepts duplicate object keys. Reject them before interpreting signed data. */
function strictJSON(raw: string) {
  const value = JSON.parse(raw);
  const stack: Array<{ object: boolean; keys: Set<string>; key: boolean }> = [];
  for (const token of raw.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]/g) ?? []) {
    if (token === "{") stack.push({ object: true, keys: new Set(), key: true });
    else if (token === "[")
      stack.push({ object: false, keys: new Set(), key: false });
    else if (token === "}" || token === "]") stack.pop();
    else if (token === ",") {
      if (stack.at(-1)?.object) stack.at(-1)!.key = true;
    } else if (token.startsWith('"') && stack.at(-1)?.key) {
      const key = JSON.parse(token);
      if (stack.at(-1)!.keys.has(key)) throw Error("Duplicate JSON key");
      stack.at(-1)!.keys.add(key);
      stack.at(-1)!.key = false;
    }
  }
  return value;
}
export async function verifyRelease(input: {
  registryURL: string;
  id: string;
  version: string;
  kid: string;
  key: JWK;
  jws: string;
  manifestRaw: string;
  descriptor: string;
}): Promise<VerifiedPluginRelease> {
  if (
    Buffer.byteLength(input.manifestRaw) > 262144 ||
    Buffer.byteLength(input.descriptor) > 262144 ||
    input.jws.length > 65536
  )
    throw Error("Release exceeds bounds");
  const verified = await compactVerify(
    input.jws,
    await importJWK(input.key, "EdDSA"),
    { algorithms: ["EdDSA"] },
  );
  if (verified.protectedHeader.kid !== input.kid)
    throw Error("Unknown signing key");
  const payload = strictJSON(Buffer.from(verified.payload).toString());
  if (
    payload.v !== 1 ||
    payload.registry !== input.registryURL ||
    payload.plugin_slug !== input.id ||
    payload.release_version !== input.version ||
    payload.kid !== input.kid
  )
    throw Error("Release identity mismatch");
  const digest = hash(input.manifestRaw);
  const asset = payload.assets?.find(
    (a: any) => a.name === input.id + "-universal.json",
  );
  if (
    payload.manifest_sha256 !== digest ||
    input.manifestRaw !== input.descriptor ||
    asset?.sha256 !== digest ||
    asset?.size !== Buffer.byteLength(input.descriptor)
  )
    throw Error("Release integrity mismatch");
  const manifest = parsePluginManifest(strictJSON(input.manifestRaw));
  if (manifest.name !== input.id || manifest.version !== input.version)
    throw Error("Manifest identity mismatch");
  return {
    registryURL: input.registryURL,
    id: input.id,
    version: input.version,
    manifestBytes: input.manifestRaw,
    manifestSha256: digest,
    image: manifest.image,
    signerKeyID: input.kid,
    manifest,
  };
}
