import assert from 'node:assert/strict';
import test from 'node:test';
import { getPayload, jwtSign, JWTAuthentication } from 'payload';

// Configuration-only initialization: no database connection or provider traffic.
Object.assign(process.env, {
  DATABASE_URL: 'postgresql://unused:unused@127.0.0.1:1/unused',
  PAYLOAD_SECRET: 'test-secret-for-sso-policy-only-at-least-32',
  SPITZLI_AUTH_ISSUER: 'https://auth.example.test/api/auth',
  SPITZLI_SSO_CLIENT_ID: 'policy-test',
  SPITZLI_SSO_CLIENT_SECRET: 'test-client-secret',
  SPITZLI_SSO_COOKIE_SECRET: 'test-cookie-secret-longer-than-thirty-two',
  SPITZLI_SSO_APP_ORIGIN: 'https://cms.example.test',
  SPITZLI_SSO_ALLOW_LOCAL_HTTP: 'false',
  SPITZLI_SSO_ENFORCE: 'true',
});

test('enforced SSO rejects native login, recovery and existing Payload JWTs without removing auth fields', async () => {
  const config = await (await import('../src/payload.config')).default;
  config.typescript.autoGenerate = false;
  const payload = await getPayload({ config, disableDBConnect: true, disableOnInit: true });
  try {
    const users = payload.collections.users.config;
    assert.equal(users.admin.hidden, true);
    for (const role of ['operator', 'admin', 'editor', 'reader']) {
      const req = { payload, user: { id: 'mapped-user', collection: 'users', role }, context: { bootstrap: true, instanceImport: true } };
      for (const operation of ['create', 'update', 'delete', 'unlock'] as const) {
        assert.equal(await users.access[operation]!({ req } as never), false, `${role}:${operation}`);
      }
      assert.ok(await users.access.read!({ req } as never));
      assert.equal(await users.access.admin!({ req } as never), true);
      const disabled = users.fields.find(field => 'name' in field && field.name === 'disabled');
      assert.ok(disabled && 'access' in disabled);
      assert.equal(await disabled.access!.update!({ req } as never), role === 'operator');
      assert.equal(await disabled.access!.create!({ req } as never), role === 'operator');
    }
    assert.deepEqual(users.auth.disableLocalStrategy, { enableFields: true, optionalPassword: true });
    assert.ok(payload.authStrategies.every(strategy => strategy.name !== 'local-jwt'));
    const fields = users.fields.flatMap(field => 'name' in field ? [field.name] : []);
    for (const field of ['email', 'hash', 'salt', 'resetPasswordToken', 'authSubject']) assert.ok(fields.includes(field), field);
    const { token } = await jwtSign({ fieldsToSign: { id: 'old-local-user', collection: 'users' }, secret: payload.secret, tokenExpiration: 300 });
    const headers = new Headers({ authorization: `JWT ${token}`, cookie: `payload-token=${token}` });
    assert.equal((await payload.auth({ headers })).user, null);
    assert.equal((await JWTAuthentication({ headers, payload })).user, null);
    await assert.rejects(() => payload.login({ collection: 'users', data: { email: 'operator@example.invalid', password: 'old-password' } }), { status: 403 });
    await assert.rejects(() => payload.forgotPassword({ collection: 'users', data: { email: 'operator@example.invalid' } }), { status: 403 });
    await assert.rejects(() => payload.resetPassword({ collection: 'users', overrideAccess: false, data: { token: 'old-reset-token', password: 'new-password' } }), { status: 403 });
    await assert.rejects(async () => {
      for (const hook of users.hooks.beforeOperation) await hook({ operation: 'refresh', req: { user: { _strategy: 'webdock-sso' } } } as never);
    }, { status: 403 });
  } finally {
    await payload.destroy();
  }
});
