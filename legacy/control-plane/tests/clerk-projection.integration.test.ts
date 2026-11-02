import { test, expect } from "bun:test";
import { randomUUID } from "node:crypto";
import { Pool } from "pg";
import { serializeSignedCookie } from "better-call";
import { assertTestDatabase } from "./helpers/database";
import { createClerkSessionBridge } from "../src/modules/auth/clerk-bridge";
import {
  enrollClerkIdentity,
  createClerkMappings,
} from "../src/modules/auth/clerk-mappings";
const url = process.env.API_TEST_DATABASE_URL;
(url ? test : test.skip)(
  "Clerk mapping preserves local identity, clamps native refresh and rejects revoked enrollment",
  async () => {
    assertTestDatabase(url!);
    const database = new Pool({ connectionString: url, max: 2 });
    let id: string | undefined;
    try {
      expect(
        (
          await database.query(
            "SELECT to_regclass('webdock_auth.clerk_identity_mapping') AS mapping",
          )
        ).rows[0].mapping,
      ).not.toBeNull();
      id = (
        await database.query<{ id: string }>(
          `INSERT INTO webdock_auth."user"(name,email,"emailVerified","mustChangePassword",role) VALUES('Clerk fixture',$1,true,false,'user') RETURNING id`,
          ["clerk-" + randomUUID() + "@example.invalid"],
        )
      ).rows[0].id;
      const issuer = "https://fixture.clerk.accounts.dev",
        subject = "user_" + randomUUID().replaceAll("-", "");
      await enrollClerkIdentity(database, {
        issuer,
        subject,
        localSubject: id,
      });
      await enrollClerkIdentity(database, {
        issuer,
        subject,
        localSubject: id,
      });
      const external = {
        issuer,
        subject,
        sessionID: "sess_" + randomUUID().replaceAll("-", ""),
      };
      const bridge = createClerkSessionBridge({
        database,
        origin: "https://auth.example.invalid",
        verifyExternal: async () => ({
          external,
          user: {
            id: subject,
            role: "user",
            banned: false,
            locked: false,
            twoFactorEnabled: false,
            emailVerified: true,
          },
          expiresAt: Date.now() + 120000,
          factorVerificationAge: [0, -1],
        }),
        signSession: async (token) =>
          (
            await serializeSignedCookie(
              "native-fixture",
              token,
              "fixture-secret-at-least-32-characters",
            )
          ).split(";")[0],
      });
      const headers = new Headers({
        authorization: "Bearer external.fixture.token",
      });
      const first = await bridge(headers),
        again = await bridge(headers);
      expect(first.actor).toEqual(again.actor);
      expect(first.actor.userID).toBe(id);
      expect(JSON.stringify(first.safe)).not.toContain("native-fixture");
      const maps = createClerkMappings(database);
      expect(await maps.localSubject(external)).toBe(id);
      expect(await maps.localSession(external)).toEqual({
        subject: id,
        sessionID: first.actor.sessionID,
      });
      await database.query(
        `UPDATE webdock_auth.session SET "expiresAt"=now()+interval '7 days' WHERE id=$1`,
        [first.actor.sessionID],
      );
      const bounded = (
        await database.query(
          `SELECT s."expiresAt"<=m.expires_at AS bounded, s."expiresAt"<=now()+interval '61 seconds' AS short FROM webdock_auth.session s JOIN webdock_auth.clerk_session_mapping m ON m.local_session_id=s.id WHERE s.id=$1`,
          [first.actor.sessionID],
        )
      ).rows[0];
      expect(bounded).toEqual({ bounded: true, short: true });
      await database.query(
        "UPDATE webdock_auth.clerk_identity_mapping SET revoked_at=now() WHERE issuer=$1 AND external_subject=$2",
        [issuer, subject],
      );
      expect(await maps.localSubject(external)).toBeNull();
      await expect(
        database.query(
          "UPDATE webdock_auth.clerk_identity_mapping SET revoked_at=NULL WHERE issuer=$1 AND external_subject=$2",
          [issuer, subject],
        ),
      ).rejects.toThrow();
      await expect(bridge(headers)).rejects.toThrow();
      await expect(
        enrollClerkIdentity(database, { issuer, subject, localSubject: id }),
      ).rejects.toThrow();
    } finally {
      if (id)
        await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
          id,
        ]);
      await database.end();
    }
  },
);
