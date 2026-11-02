import { configureOAuthSession } from '@webdock/oauth-session'
import assert from 'node:assert/strict'
import { afterEach, test } from 'node:test'
import { adminSSORedirect, safeAdminReturnTo, configurePayloadSSO, intersectRoles, validateIdentity } from './index.ts'

// Each test may replace fetch; the real one comes back afterwards.
const realFetch = globalThis.fetch
const stubFetch = (stub: (input: Request | URL | string, init?: RequestInit) => Promise<Response>) => { globalThis.fetch = stub as typeof fetch }
afterEach(() => { globalThis.fetch = realFetch })

test('Studio return paths are explicitly allowed without widening ordinary CMS redirects', () => {
  const paths = ['/tenants/123/mail', '/offers/opaque-token', '/people?q=person'];
  for (const path of paths) {
    assert.equal(safeAdminReturnTo(path), null);
    assert.equal(safeAdminReturnTo(path, ['/tenants', '/offers', '/people']), path);
  }
  for (const path of ['//evil.test', '/tenants-evil', '/tenants/../api/auth', '/offers/%2f%2fevil.test', '/api/sso/login']) {
    assert.equal(safeAdminReturnTo(path, ['/tenants', '/offers']), null);
  }
});

test('admin return targets reject external URLs, encoded escapes and native auth loops', () => {
  for (const path of ['/cms', '/cms?module=pages&id=42', '/system/collections/pages/42', '/admin', '/admin/collections/pages/42?locale=de&depth=0', '/admin/globals/settings#title']) {
    assert.equal(safeAdminReturnTo(path), path)
  }
  for (const path of [undefined, [], '/cms-evil', '/cms/../../outside', '/system/login', '/cms/%2f/evil', 'https://evil.test/admin', '//evil.test/admin', '/administrator',
    '/admin//login', '/admin/../../outside', '/admin/../login', '/admin\\evil', '/admin/%2f/evil', '/admin/%5cevil',
    '/admin/%252flogin', '/admin/%00', '/admin/%', '/admin/login', '/admin/%6cogin',
    '/admin/login/nested?redirect=/admin', '/admin/logout', '/admin/reset/token', '/admin/forgot',
    '/admin/create-first-user', '/admin/unauthorized', '/admin/\nlogin']) {
    assert.equal(safeAdminReturnTo(path), null, String(path))
  }
})

test('only enforced sites bypass native login and local account management', () => {
  assert.equal(adminSSORedirect(true, ['login'], { redirect: '?locale=de' }), '/api/sso/login?returnTo=%2Fadmin%3Flocale%3Dde')
  const target = '/admin/collections/pages/42?locale=de'
  assert.equal(adminSSORedirect(true, ['login'], { redirect: target }), `/api/sso/login?${new URLSearchParams({ returnTo: target })}`)
  for (const route of ['login', 'forgot', 'reset', 'create-first-user']) {
    assert.equal(adminSSORedirect(false, [route]), null)
    assert.equal(adminSSORedirect(true, [route], { redirect: '//evil.test' }), '/api/sso/login?returnTo=%2Fadmin')
  }
  for (const segments of [['account'], ['collections', 'users', '42']]) {
    assert.equal(adminSSORedirect(true, segments), '/admin')
    assert.equal(adminSSORedirect(false, segments), null)
  }
  for (const segments of [[], ['globals', 'settings'], ['logout'], ['unauthorized']]) {
    assert.equal(adminSSORedirect(true, segments), null)
  }
})

const now = 1_800_000_000
const claims = { active: true, client_id: 'cms-one', sub: 'subject-123', exp: now + 300, webdock_role: 'operator' }

test('introspection rejects inactive, cross-client, expired and malformed claims', () => {
  assert.equal(validateIdentity(claims, 'cms-one', now)?.sub, 'subject-123')
  assert.equal(validateIdentity({ ...claims, exp: now + 28800 }, 'cms-one', now)?.exp, now + 28800)
  for (const invalid of [null, {}, { ...claims, active: false }, { ...claims, client_id: 'cms-two' },
    { ...claims, exp: now }, { ...claims, exp: now + 28801 }, { ...claims, sub: '' },
    { ...claims, webdock_role: 'owner' }, { ...claims, disabled: true }, { ...claims, exp: '1800000300' }]) {
    assert.equal(validateIdentity(invalid, 'cms-one', now), null)
  }
})

test('role intersection never promotes local or central permissions', () => {
  assert.equal(intersectRoles('operator', 'operator'), 'operator')
  assert.equal(intersectRoles('operator', 'editor'), 'editor')
  assert.equal(intersectRoles('admin', 'operator'), 'admin')
  assert.equal(intersectRoles('reader', 'operator'), 'reader')
  assert.equal(intersectRoles('owner', 'operator'), null)
})

const options = {
  issuer: 'https://auth.example.test/api/auth', clientId: 'cms-one', clientSecret: 'secret',
  cookieSecret: 'a-secret-at-least-thirty-two-characters-long', appOrigin: 'https://cms.example.test',
  getPayload: async () => { throw new Error('No account lookup should happen') },
}

test('public requests and malformed cookies are anonymous without calling provider or database', async () => {
  const sso = configurePayloadSSO(options)
  for (const cookie of ['', '__Host-webdock-sso=garbage', '__Host-webdock-sso=%']) {
    const result = await sso.strategy.authenticate({ headers: new Headers({ cookie }), payload: {} as never })
    assert.equal(result.user, null)
  }
})

test('callback without valid flow cookie fails closed and clears cookies', async () => {
  const response = await configurePayloadSSO(options).callback(new Request(`${options.appOrigin}/api/sso/callback?code=fake&state=fake`))
  assert.equal(response.status, 401)
  assert.match(response.headers.get('set-cookie') ?? '', /Max-Age=0/)
})

test('rejects insecure configuration and external redirect paths', () => {
  for (const override of [{ issuer: 'http://auth.example.test' }, { appOrigin: 'http://cms.example.test' },
    { successPath: '//evil.test' }, { logoutPath: '/\\evil.test' }, { cookieSecret: 'short' }]) {
    assert.throws(() => configurePayloadSSO({ ...options, ...override }))
  }
})

test('OIDC flow verifies signed identity, introspects every request, and never links by email', async () => {
  const { generateKeyPairSync, sign } = await import('node:crypto')
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 })
  const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'test-key', alg: 'RS256', use: 'sig' }
  const jwt = (body: object) => {
    const message = [ { alg: 'RS256', kid: 'test-key' }, body ].map(value => Buffer.from(JSON.stringify(value)).toString('base64url')).join('.')
    return `${message}.${sign('RSA-SHA256', Buffer.from(message), privateKey).toString('base64url')}`
  }
  let nonce = ''
  let introspections = 0
  let active = true
  let unavailable = false
  let badSignature = false
  let subject = 'subject-123'
  let extraClaims: Record<string, unknown> = {}
  let mapped = true
  let centralRole = claims.webdock_role
  const issuer = options.issuer
  const json = (value: object) => new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } })
  stubFetch(async (input: Request | URL | string, init?: RequestInit) => {
    const url = String(input)
    if (url.includes('.well-known/openid-configuration')) return json({
      issuer, authorization_endpoint: `${issuer}/oauth2/authorize`, token_endpoint: `${issuer}/oauth2/token`,
      introspection_endpoint: `${issuer}/oauth2/introspect`, jwks_uri: `${issuer}/jwks`,
      end_session_endpoint: `${issuer}/oauth2/end-session`,
      response_types_supported: ['code'], subject_types_supported: ['public'], id_token_signing_alg_values_supported: ['RS256'],
    })
    if (url === `${issuer}/jwks`) return json({ keys: [jwk] })
    if (url === `${issuer}/oauth2/token`) {
      assert.ok(String(init?.body).includes('code_verifier='))
      let idToken = jwt({ iss: issuer, aud: options.clientId, sub: 'subject-123', exp: Math.floor(Date.now() / 1000) + 300,
        iat: Math.floor(Date.now() / 1000), nonce, ...extraClaims })
      if (badSignature) {
        const parts = idToken.split('.')
        parts[2] = Buffer.alloc(256).toString('base64url')
        idToken = parts.join('.')
      }
      return json({ token_type: 'Bearer', access_token: 'opaque-access-token', expires_in: 300, id_token: idToken })
    }
    if (url === `${issuer}/oauth2/introspect`) {
      introspections++
      if (unavailable) throw new Error('Provider unavailable')
      const body = new URLSearchParams(String(init?.body))
      assert.equal(body.get('client_id'), options.clientId)
      assert.equal(body.get('client_secret'), options.clientSecret)
      return json({ ...claims, active, sub: subject, webdock_role: centralRole, email: 'same-email@example.test', exp: Math.floor(Date.now() / 1000) + 300 })
    }
    throw new Error(`Unexpected test provider URL: ${url}`)
  })
  const payload = { find: async (query: { where: unknown }) => {
    assert.deepEqual(query.where, { authSubject: { equals: subject } })
    return { docs: mapped ? [{ id: 'local-123', email: 'different@example.test', authSubject: subject, role: 'editor' }] : [] }
  } } as never
  const sso = configurePayloadSSO({ ...options, getPayload: async () => payload })
  const begin = async (returnTo = '', instance = sso) => {
    const login = await instance.login(new Request(`${options.appOrigin}/api/sso/login?${new URLSearchParams({ returnTo })}`))
    assert.equal(login.status, 302)
    const location = new URL(login.headers.get('location')!)
    assert.equal(location.searchParams.get('code_challenge_method'), 'S256')
    assert.equal(location.searchParams.get('scope'), 'openid profile email')
    assert.equal(location.searchParams.has('resource'), false)
    nonce = location.searchParams.get('nonce')!
    const setCookie = login.headers.get('set-cookie')!
    assert.match(setCookie, /HttpOnly; Secure; SameSite=Lax/)
    assert.ok(!setCookie.includes('Domain='))
    return new Request(`${options.appOrigin}/api/sso/callback?code=test-code&state=${location.searchParams.get('state')}`, {
      headers: { cookie: setCookie.split(';')[0]! },
    })
  }
  const target = '/admin/collections/pages/42?locale=de&depth=0'
  const requested = await begin(target)
  // Only the encrypted flow controls this target, never callback query parameters.
  const callback = await sso.callback(new Request(`${requested.url}&returnTo=https://evil.test`, requested))
  assert.equal(callback.headers.get('location'), `${options.appOrigin}${target}`)
  for (const invalid of ['//evil.test', '/admin/login', '/outside']) {
    assert.equal((await sso.callback(await begin(invalid))).headers.get('location'), `${options.appOrigin}/admin`)
  }
  introspections = 1
  assert.equal(callback.status, 302)
  assert.ok(callback.headers.getSetCookie().some(cookie => cookie.startsWith('payload-token=;') && cookie.includes('Max-Age=0')))
  const headers = new Headers({ cookie: callback.headers.getSetCookie().find(value => value.startsWith('__Host-webdock-sso='))!.split(';')[0]! })
  const first = await sso.strategy.authenticate({ headers, payload })
  assert.equal(first.user?.id, 'local-123')
  assert.equal(first.user?._strategy, 'webdock-sso')
  const refresh = await sso.refresh(new Request(`${options.appOrigin}/api/users/refresh-token`, { method: 'POST', headers }))
  assert.equal(refresh.status, 200)
  assert.equal(refresh.headers.has('set-cookie'), false)
  const refreshed = await refresh.json()
  assert.equal(refreshed.token, undefined)
  assert.equal(refreshed.refreshedToken, undefined)
  assert.equal(refreshed.user.role, 'editor')
  assert.ok(refreshed.exp <= Math.floor(Date.now() / 1000) + 300)
  assert.equal((first.user as { role?: string })?.role, 'editor')
  assert.equal(introspections, 3)
  active = false
  assert.equal((await sso.strategy.authenticate({ headers, payload })).user, null)
  assert.equal(introspections, 4)
  active = true
  unavailable = true
  assert.equal((await sso.strategy.authenticate({ headers, payload })).user, null)
  unavailable = false
  const duplicateHeaders = new Headers({ cookie: `${headers.get('cookie')}; ${headers.get('cookie')}` })
  assert.equal((await sso.strategy.authenticate({ headers: duplicateHeaders, payload })).user, null)
  mapped = false
  assert.equal((await sso.callback(await begin())).status, 401)
  const portal = configurePayloadSSO({ ...options, getPayload: async () => payload, allowUnmappedPortalUsers: true, returnToPrefixes: ['/offers', '/tenants'] })
  assert.equal((await portal.callback(await begin('/offers/test-token', portal))).status, 401, 'operators still require their local mapping')
  centralRole = 'reader'
  const portalCallback = await portal.callback(await begin('/offers/test-token', portal))
  assert.equal(portalCallback.status, 302)
  assert.equal(portalCallback.headers.get('location'), `${options.appOrigin}/offers/test-token`)
  const portalHeaders = new Headers({ cookie: portalCallback.headers.getSetCookie().find(value => value.startsWith('__Host-webdock-sso='))!.split(';')[0]! })
  assert.equal((await portal.getDelegatedSession(portalHeaders))?.subject, 'subject-123')
  assert.equal((await portal.strategy.authenticate({ headers: portalHeaders, payload })).user, null, 'portal identity cannot bypass native Payload mapping')
  active = false
  assert.equal(await portal.getDelegatedSession(portalHeaders), null)
  active = true
  centralRole = claims.webdock_role
  mapped = true
  subject = 'wrong-subject'
  assert.equal((await sso.callback(await begin())).status, 401)
  subject = 'subject-123'
  for (const invalid of [{ iss: 'https://evil.test' }, { aud: 'other-client' }, { nonce: 'wrong-nonce' }, { exp: 1 }]) {
    extraClaims = invalid
    assert.equal((await sso.callback(await begin())).status, 401)
  }
  extraClaims = {}
  badSignature = true
  assert.equal((await sso.callback(await begin())).status, 401)
  badSignature = false
  const badState = await begin()
  assert.equal((await sso.callback(new Request(badState.url.replace(/state=[^&]+/, 'state=wrong'), badState))).status, 401)
  const logout = await sso.logout(new Request(`${options.appOrigin}/api/sso/logout`, { method: 'POST', headers: { origin: options.appOrigin } }))
  assert.equal(logout.status, 303)
  assert.equal(logout.headers.get('location'), `${options.appOrigin}/login`)
  assert.ok(logout.headers.getSetCookie().every(cookie => cookie.includes('Max-Age=0')))
  assert.equal((await sso.logout(new Request(`${options.appOrigin}/api/sso/logout`))).status, 403)
  const central = configurePayloadSSO({ ...options, getPayload: async () => payload, centralLogout: true })
  const centralCallback = await central.callback(await begin('', central))
  assert.equal(centralCallback.status, 302)
  const centralCookie = centralCallback.headers.getSetCookie().find(value => value.startsWith('__Host-webdock-sso='))!.split(';')[0]!
  const centralLogout = await central.logout(new Request(`${options.appOrigin}/api/sso/logout`, { method: 'POST', headers: { origin: options.appOrigin, cookie: centralCookie } }))
  const destination = new URL(centralLogout.headers.get('location')!)
  assert.equal(destination.pathname, '/api/auth/oauth2/end-session')
  assert.ok(destination.searchParams.get('id_token_hint'), 'The callback retains the verified ID token for immediate browser logout')
})

test('local HTTP is explicitly gated and forbidden for production or remote hosts', async () => {
  const local = { ...options, issuer: 'http://localhost:3125/api/auth', appOrigin: 'http://127.0.0.1:3120', allowLocalHTTP: true }
  assert.doesNotThrow(() => configurePayloadSSO(local))
  assert.throws(() => configurePayloadSSO({ ...local, allowLocalHTTP: false }))
  for (const override of [{ issuer: 'http://auth.example.test/api/auth' }, { appOrigin: 'http://cms.example.test' },
    { appOrigin: 'http://localhost.evil.test' }, { issuer: 'http://192.168.1.1/api/auth' }]) {
    assert.throws(() => configurePayloadSSO({ ...local, ...override }))
  }
  const env: Record<string, string | undefined> = process.env
  const previous = env.NODE_ENV
  try {
    env.NODE_ENV = 'production'
    assert.throws(() => configurePayloadSSO(local))
  } finally {
    if (previous === undefined) delete env.NODE_ENV
    else env.NODE_ENV = previous
  }
})

test('central logout reaches the provider for new and pre-upgrade browser sessions', async () => {
  const { sealData } = await import('iron-session')
  const endpoint = `${options.issuer}/oauth2/end-session`
  stubFetch(async () => Response.json({
    issuer: options.issuer, end_session_endpoint: endpoint, jwks_uri: `${options.issuer}/jwks`,
    introspection_endpoint: `${options.issuer}/oauth2/introspect`,
    authorization_endpoint: `${options.issuer}/oauth2/authorize`, token_endpoint: `${options.issuer}/oauth2/token`,
    response_types_supported: ['code'], subject_types_supported: ['public'], id_token_signing_alg_values_supported: ['RS256'],
  }))
  const sso = configurePayloadSSO({ ...options, centralLogout: true })
  for (const idToken of ['signed-id-token', undefined]) {
    const sealed = await sealData({ kind: 'session', accessToken: 'opaque', sub: 'subject-123', exp: Math.floor(Date.now() / 1000) + 300, idToken }, { password: options.cookieSecret, ttl: 300 })
    const response = await sso.logout(new Request(`${options.appOrigin}/api/sso/logout`, {
      method: 'POST', headers: { origin: options.appOrigin, cookie: `__Host-webdock-sso=${sealed}` },
    }))
    assert.equal(response.status, 303)
    const destination = new URL(response.headers.get('location')!)
    assert.equal(destination.origin + destination.pathname, endpoint)
    assert.equal(destination.searchParams.get('client_id'), options.clientId)
    assert.equal(destination.searchParams.get('id_token_hint'), idToken ?? null)
    assert.equal(destination.searchParams.get('post_logout_redirect_uri'), `${options.appOrigin}/login`)
    assert.ok(response.headers.getSetCookie().every(cookie => cookie.includes('Max-Age=0')))
  }
  const missing = await sso.logout(new Request(`${options.appOrigin}/api/sso/logout`, { method: 'POST', headers: { origin: options.appOrigin } }))
  assert.equal(new URL(missing.headers.get('location')!).pathname, '/api/auth/oauth2/end-session')
  const foreign = await sso.logout(new Request(`${options.appOrigin}/api/sso/logout`, { method: 'POST', headers: { origin: 'https://other.example.test' } }))
  assert.equal(foreign.status, 403)
  assert.equal(foreign.headers.has('set-cookie'), false)
  stubFetch(async () => { throw Error('Provider unavailable') })
  const unavailable = await configurePayloadSSO({ ...options, centralLogout: true }).logout(new Request(`${options.appOrigin}/api/sso/logout`, { method: 'POST', headers: { origin: options.appOrigin } }))
  assert.equal(unavailable.headers.get('location'), `${options.appOrigin}/login`)
  assert.ok(unavailable.headers.getSetCookie().every(cookie => cookie.includes('Max-Age=0')), 'Provider downtime must still clear local credentials')
})

test('local HTTP login uses unprefixed, non-Secure development cookies', async () => {
  const issuer = 'http://localhost:3125/api/auth'
  stubFetch(async () => new Response(JSON.stringify({
    issuer, authorization_endpoint: `${issuer}/oauth2/authorize`, token_endpoint: `${issuer}/oauth2/token`,
    introspection_endpoint: `${issuer}/oauth2/introspect`, jwks_uri: `${issuer}/jwks`,
    response_types_supported: ['code'], subject_types_supported: ['public'], id_token_signing_alg_values_supported: ['RS256'],
  }), { headers: { 'content-type': 'application/json' } }))
  const sso = configurePayloadSSO({ ...options, issuer, appOrigin: 'http://127.0.0.1:3120', allowLocalHTTP: true })
  const response = await sso.login(new Request('http://127.0.0.1:3120/api/sso/login'))
  assert.equal(response.status, 302)
  const cookie = response.headers.get('set-cookie')!
  assert.match(cookie, /^webdock-sso-flow=/)
  assert.match(cookie, /HttpOnly; SameSite=Lax/)
  assert.ok(!cookie.includes('Secure'))
  assert.ok(!cookie.includes('Domain='))
})


test('SSO collection hooks block native JWT refresh and preserve effective role on me', async () => {
  const sso = configurePayloadSSO(options)
  const req = { user: { _strategy: 'webdock-sso', role: 'editor', _ssoExp: now + 300 } }
  await assert.rejects(() => sso.hooks.beforeOperation![0]!({ operation: 'refresh', req } as never) as Promise<void>)
  const me = await sso.hooks.me![0]!({ args: { req }, user: { role: 'operator' } } as never)
  assert.equal(me?.user.role, 'editor')
  assert.equal(me?.exp, now + 300)
})

test('callback accepts a proxy-normalized URL only with the registered raw Host and uses the exact redirect URI', async () => {
  const { generateKeyPairSync, sign } = await import('node:crypto')
  const { sealData } = await import('iron-session')
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 })
  const issuer = 'http://localhost:3125/api/auth'
  const appOrigin = 'http://127.0.0.1:3120'
  const exp = Math.floor(Date.now() / 1000) + 300
  const flow = { kind: 'flow', verifier: 'a'.repeat(43), state: 'test-state', nonce: 'test-nonce', exp }
  const cookie = `webdock-sso-flow=${await sealData(flow, { password: options.cookieSecret, ttl: 300 })}`
  const input = [{ alg: 'RS256', kid: 'key' }, { iss: issuer, aud: options.clientId, sub: 'subject-123', nonce: flow.nonce, iat: exp - 300, exp }]
    .map(value => Buffer.from(JSON.stringify(value)).toString('base64url')).join('.')
  const idToken = `${input}.${sign('RSA-SHA256', Buffer.from(input), privateKey).toString('base64url')}`
  const json = (value: object) => Response.json(value)
  stubFetch(async (input: Request | URL | string, init?: RequestInit) => {
    const url = String(input)
    if (url.includes('.well-known/openid-configuration')) return json({ issuer,
      authorization_endpoint: `${issuer}/oauth2/authorize`, token_endpoint: `${issuer}/oauth2/token`,
      introspection_endpoint: `${issuer}/oauth2/introspect`, jwks_uri: `${issuer}/jwks`,
      response_types_supported: ['code'], subject_types_supported: ['public'], id_token_signing_alg_values_supported: ['RS256'],
    })
    if (url.endsWith('/jwks')) return json({ keys: [{ ...publicKey.export({ format: 'jwk' }), kid: 'key', alg: 'RS256' }] })
    if (url.endsWith('/oauth2/token')) {
      assert.equal(new URLSearchParams(String(init?.body)).get('redirect_uri'), `${appOrigin}/api/sso/callback`)
      return json({ access_token: 'opaque', token_type: 'Bearer', expires_in: 300, id_token: idToken })
    }
    if (url.endsWith('/oauth2/introspect')) return json({ ...claims, exp })
    throw Error('Unexpected URL')
  })
  const sso = configurePayloadSSO({ ...options, issuer, appOrigin, allowLocalHTTP: true,
    getPayload: async () => ({ find: async () => ({ docs: [{ id: 'local', authSubject: 'subject-123', role: 'operator' }] }) }) as never,
  })
  const internalURL = 'http://localhost:3120/api/sso/callback?code=test&state=test-state'
  assert.equal((await sso.callback(new Request(internalURL, { headers: { host: '127.0.0.1:3120', cookie } }))).status, 302)
  assert.equal((await sso.callback(new Request(internalURL, { headers: { host: 'evil.test', 'x-forwarded-host': '127.0.0.1:3120', cookie } }))).status, 401)
  assert.equal((await sso.callback(new Request(internalURL.replace('/api/sso/callback', '/other'), { headers: { host: '127.0.0.1:3120', cookie } }))).status, 401)
})

test('portal mapping can use the verified central session without a Payload runtime', async () => {
  const { sealData } = await import('iron-session')
  const exp = Math.floor(Date.now() / 1000) + 300
  const cookie = `__Host-webdock-sso=${await sealData({ kind: 'session', accessToken: 'opaque', sub: 'subject-123', exp }, { password: options.cookieSecret, ttl: 28800 })}`
  let active = true
  stubFetch(async (input: Request | URL | string) => {
    if (String(input).includes('.well-known/openid-configuration')) return Response.json({issuer: options.issuer,
      authorization_endpoint: options.issuer+'/oauth2/authorize', token_endpoint: options.issuer+'/oauth2/token', introspection_endpoint: options.issuer+'/oauth2/introspect', jwks_uri: options.issuer+'/jwks',
      response_types_supported:['code'],subject_types_supported:['public'],id_token_signing_alg_values_supported:['RS256']})
    return Response.json({...claims, active, sid:'central-session', exp})
  })
  const sso = configurePayloadSSO({ ...options, getPayload: undefined,
    lookupUser: async identity => {
      assert.equal(identity.sessionID, 'central-session')
      return { id:'local',authSubject:identity.sub,role:'operator',name:'Operator',email:'operator@example.invalid',createdAt:'2026-01-01T00:00:00Z',updatedAt:'2026-01-01T00:00:00Z' }
    },
  })
  assert.equal((await sso.getDelegatedSession(new Headers({cookie})))?.sessionID,'central-session')
  assert.equal((await sso.strategy.authenticate({headers:new Headers({cookie}),payload:{} as never})).user,null,'API mappings cannot be used as a Payload authentication strategy')
  active = false
  assert.equal(await sso.getDelegatedSession(new Headers({cookie})),null)
})


test('API-local introspection preserves client, subject, expiry and revocation checks on every read', async () => {
  const { sealData } = await import('iron-session')
  const expires = Math.floor(Date.now() / 1000) + 300
  const stored = { kind: 'session', accessToken: 'local-token', sub: 'subject-123', exp: expires }
  const cookie = '__Host-webdock-sso=' + await sealData(stored, { password: options.cookieSecret, ttl: 300 })
  let calls = 0
  let result: unknown = { ...claims, exp: expires, sid: 'native-session', webdock_role: 'reader' }
  const sso = configureOAuthSession({ ...options, lookupUser: async () => null, allowUnmappedPortalUsers: true,
    introspect: async (token: string) => { assert.equal(token, stored.accessToken); calls++; return result },
  })
  const headers = new Headers({ cookie })
  assert.equal((await sso.getDelegatedSession(headers))?.sessionID, 'native-session')
  for (const override of [{ active: false }, { client_id: 'another-client' }, { sub: 'another-subject' }, { exp: expires - 301 }, { disabled: true }]) {
    result = { ...claims, exp: expires, webdock_role: 'reader', ...override }
    assert.equal(await sso.getDelegatedSession(headers), null)
  }
  assert.equal(calls, 6)
})
