import test from "node:test";
import assert from "node:assert/strict";
import {
  createHash,
  generateKeyPairSync,
  randomBytes,
  sign,
} from "node:crypto";
import { betterAuth } from "better-auth";
import { memoryAdapter } from "better-auth/adapters/memory";
import {
  passkeySecurity,
  passwordTwoFactor,
} from "../../src/modules/auth/passkeys";

const origin = "http://localhost:3125";
const password = "Local disposable password 123!";

async function fixture() {
  const data: Record<string, Record<string, unknown>[]> = {
    user: [],
    account: [],
    session: [],
    verification: [],
    passkey: [],
    twoFactor: [],
  };
  const auth = betterAuth({
    baseURL: origin,
    secret: randomBytes(32).toString("hex"),
    database: memoryAdapter(data),
    logger: { disabled: true },
    emailAndPassword: { enabled: true },
    user: {
      additionalFields: {
        role: { type: "string", defaultValue: "user", input: false },
        mustChangePassword: {
          type: "boolean",
          defaultValue: false,
          input: false,
        },
      },
    },
    plugins: [passwordTwoFactor(), passkeySecurity(origin)],
  });
  let cookies = new Map<string, string>();
  const call = async (path: string, body?: unknown) => {
    const response = await auth.handler(
      new Request(`${origin}/api/auth${path}`, {
        method: body ? "POST" : "GET",
        headers: {
          Origin: origin,
          "Content-Type": "application/json",
          Cookie: [...cookies.values()].join("; "),
        },
        body: body ? JSON.stringify(body) : undefined,
      }),
    );
    for (const cookie of response.headers.getSetCookie()) {
      const value = cookie.split(";")[0];
      cookies.set(value.split("=")[0], value);
    }
    return response;
  };
  assert.equal(
    (
      await call("/sign-up/email", {
        email: "passkey@example.invalid",
        name: "Passkey test",
        password,
      })
    ).status,
    200,
  );
  data.user[0].emailVerified = true;
  const keys = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
  const jwk = keys.publicKey.export({ format: "jwk" });
  // COSE EC2 / ES256 / P-256 public key: {1:2,3:-7,-1:1,-2:x,-3:y}.
  const publicKey = Buffer.concat([
    Buffer.from("a5010203262001215820", "hex"),
    Buffer.from(jwk.x!, "base64url"),
    Buffer.from("225820", "hex"),
    Buffer.from(jwk.y!, "base64url"),
  ]).toString("base64url");
  const credentialID = randomBytes(32).toString("base64url");
  data.passkey.push({
    id: "test-passkey",
    userId: data.user[0].id,
    publicKey,
    credentialID,
    counter: 0,
    deviceType: "singleDevice",
    backedUp: false,
    name: "Disposable key",
  });
  let counter = 0;
  const authenticate = async (verified = true, clientOrigin = origin) => {
    const options = await (
      await call("/passkey/generate-authenticate-options")
    ).json();
    assert.equal(options.userVerification, "required");
    const clientData = Buffer.from(
      JSON.stringify({
        type: "webauthn.get",
        challenge: options.challenge,
        origin: clientOrigin,
      }),
    );
    const authData = Buffer.alloc(37);
    createHash("sha256").update("localhost").digest().copy(authData);
    authData[32] = verified ? 5 : 1;
    authData.writeUInt32BE(++counter, 33);
    const signature = sign(
      "sha256",
      Buffer.concat([
        authData,
        createHash("sha256").update(clientData).digest(),
      ]),
      keys.privateKey,
    );
    return call("/passkey/verify-authentication", {
      response: {
        id: credentialID,
        rawId: credentialID,
        type: "public-key",
        clientExtensionResults: {},
        response: {
          clientDataJSON: clientData.toString("base64url"),
          authenticatorData: authData.toString("base64url"),
          signature: signature.toString("base64url"),
        },
      },
    });
  };
  const register = async (verified = true) => {
    const options = await (
      await call("/passkey/generate-register-options")
    ).json();
    const clientData = Buffer.from(
      JSON.stringify({
        type: "webauthn.create",
        challenge: options.challenge,
        origin,
      }),
    );
    const authData = Buffer.concat([
      createHash("sha256").update("localhost").digest(),
      Buffer.from([verified ? 0x45 : 0x41]),
      Buffer.alloc(4),
      Buffer.alloc(16),
      Buffer.from([0, 32]),
      randomBytes(32),
      Buffer.from(publicKey, "base64url"),
    ]);
    // CBOR none attestation: {fmt:"none",attStmt:{},authData:bytes}.
    const attestation = Buffer.concat([
      Buffer.from(
        "a363666d74646e6f6e656761747453746d74a068617574684461746158",
        "hex",
      ),
      Buffer.from([authData.length]),
      authData,
    ]);
    const id = authData.subarray(55, 87).toString("base64url");
    return call("/passkey/verify-registration", {
      name: "New device",
      response: {
        id,
        rawId: id,
        type: "public-key",
        clientExtensionResults: {},
        response: {
          clientDataJSON: clientData.toString("base64url"),
          attestationObject: attestation.toString("base64url"),
          transports: ["internal"],
        },
      },
    });
  };
  return {
    data,
    call,
    authenticate,
    register,
    clearCookies: () => {
      cookies = new Map();
    },
  };
}

test("passkeys require verified users and completed operator setup before enrollment", async () => {
  const f = await fixture();
  const options = await (
    await f.call("/passkey/generate-register-options")
  ).json();
  assert.equal(options.rp.id, "localhost");
  assert.equal(options.authenticatorSelection.userVerification, "required");
  f.data.user[0].mustChangePassword = true;
  assert.equal(
    (await f.call("/passkey/generate-register-options")).status,
    403,
  );
  assert.equal(
    (await f.call("/passkey/verify-registration", { response: {} })).status,
    403,
  );
  f.data.user[0].mustChangePassword = false;
  f.data.user[0].role = "operator";
  assert.equal(
    (await f.call("/passkey/generate-register-options")).status,
    403,
  );
  f.clearCookies();
  assert.equal(
    (await f.call("/passkey/generate-register-options")).status,
    401,
  );
});

test("signed passkey assertions enforce origin and user verification", async () => {
  const f = await fixture();
  f.clearCookies();
  assert.equal((await f.authenticate(false)).status, 403);
  assert.equal(await (await f.call("/get-session")).json(), null);
  assert.equal(
    (await f.authenticate(true, "https://attacker.invalid")).status,
    400,
  );
  assert.equal(await (await f.call("/get-session")).json(), null);
  assert.equal((await f.authenticate()).status, 200);
  assert.ok((await (await f.call("/get-session")).json()).session);
  f.data.user[0].emailVerified = false;
  f.clearCookies();
  assert.equal((await f.authenticate()).status, 403);
});

test("verified passkeys sign in directly while passwords still require MFA", async () => {
  const f = await fixture();
  // Use real enrollment to obtain disposable encrypted recovery codes.
  const enrollment = await (
    await f.call("/two-factor/enable", { password })
  ).json();
  assert.equal(enrollment.backupCodes.length, 10);
  f.data.user[0].twoFactorEnabled = true;
  f.data.twoFactor[0].verified = true;
  f.clearCookies();
  f.data.user[0].role = "operator";
  assert.equal((await f.authenticate(false)).status, 403);
  assert.equal(await (await f.call("/get-session")).json(), null);
  const response = await f.authenticate();
  assert.equal(response.status, 200);
  assert.equal((await response.json()).twoFactorRedirect, undefined);
  const session = await (await f.call("/get-session")).json();
  assert.equal(session.user.id, f.data.user[0].id);
  assert.equal(session.user.twoFactorEnabled, true);
  await f.call("/sign-out", {});
  f.clearCookies();
  const passwordResponse = await f.call("/sign-in/email", {
    email: "passkey@example.invalid",
    password,
  });
  assert.equal(passwordResponse.status, 200);
  assert.deepEqual(await passwordResponse.json(), {
    twoFactorRedirect: true,
    twoFactorMethods: ["totp"],
  });
  assert.equal(await (await f.call("/get-session")).json(), null);
  assert.equal(
    (
      await f.call("/two-factor/verify-backup-code", {
        code: enrollment.backupCodes[0],
        trustDevice: false,
      })
    ).status,
    200,
  );
  assert.ok((await (await f.call("/get-session")).json()).session);
});

test("deletion requires a password fallback and preserves other users' credentials", async () => {
  const f = await fixture();
  f.data.passkey.push({
    ...f.data.passkey[0],
    id: "other-passkey",
    userId: "other-user",
  });
  assert.equal(
    (await f.call("/passkey/delete-passkey", { id: "other-passkey" })).status,
    401,
  );
  const accounts = f.data.account.splice(0);
  assert.equal(
    (await f.call("/passkey/delete-passkey", { id: "test-passkey" })).status,
    403,
  );
  assert.equal(f.data.passkey.length, 2);
  f.data.account.push(...accounts);
  assert.equal(
    (await f.call("/passkey/delete-passkey", { id: "test-passkey" })).status,
    200,
  );
  assert.equal(f.data.passkey.length, 1);
});

test("registration verifies device confirmation and saves a named passkey", async () => {
  const f = await fixture();
  assert.equal((await f.register(false)).status, 403);
  assert.equal(f.data.passkey.length, 1);
  assert.equal((await f.register()).status, 200);
  const keys = await (await f.call("/passkey/list-user-passkeys")).json();
  assert.equal(keys.length, 2);
  assert.ok(keys.some((key: { name: string }) => key.name === "New device"));
});
