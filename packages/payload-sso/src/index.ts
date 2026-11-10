import type { CollectionConfig, Field, Payload } from 'payload'
import { configureOAuthSession, safeAdminReturnTo, type Identity, type OAuthSessionOptions } from '@spitzli/oauth-session'
export { safeAdminReturnTo, intersectRoles, validateIdentity } from '@spitzli/oauth-session'
/** Payload preserves deep links in `redirect` when sending anonymous requests to login. */
export function adminSSORedirect(enforced: boolean, segments: string[] = [], searchParams: Record<string, unknown> = {}): string | null {
  if (!enforced) return null
  if (segments[0] === 'account' || (segments[0] === 'collections' && segments[1] === 'users')) return '/admin'
  if (!['login', 'forgot', 'reset', 'create-first-user'].includes(segments[0] ?? '')) return null
  const requested = searchParams.redirect
  const returnTo = safeAdminReturnTo(typeof requested === 'string' && requested.startsWith('?') ? `/admin${requested}` : requested) ?? '/admin'
  return `/api/sso/login?${new URLSearchParams({ returnTo })}`
}

/** Set only through an explicit offline bootstrap with overrideAccess, never by an account editor. */
export const authSubjectField: Field = {
  name: 'authSubject', type: 'text', unique: true, index: true,
  admin: { readOnly: true }, access: { create: () => false, update: () => false },
}

export type PayloadSSOOptions = OAuthSessionOptions<{ id: string; authSubject: string; role: string; disabled?: boolean; name: string; email: string; createdAt: string; updatedAt: string }> & { getPayload?: () => Promise<Payload> }

/** Payload owns collection lookup and hooks; OAuth transport is shared with other hosts. */
export function configurePayloadSSO(options: PayloadSSOOptions) {
  if (!options.getPayload && !options.lookupUser) throw new Error('A user mapping is required')
  const findPayloadUser = async (payload: Payload | undefined, identity: Identity) => {
    if (!payload) return null
    const result = await payload.find({ collection: 'users', where: { authSubject: { equals: identity.sub } }, limit: 2, depth: 0, overrideAccess: true })
    return result.docs.length === 1 ? result.docs[0] : null
  }
  const session = configureOAuthSession<object>({ ...options,
    lookupUser: options.lookupUser ?? (async identity => findPayloadUser(await options.getPayload?.(), identity)),
  })
  const hooks: NonNullable<CollectionConfig['hooks']> = {
    beforeOperation: [async ({ operation, req }) => {
      if (operation === 'refresh' && req.user?._strategy === 'webdock-sso') {
        const { APIError } = await import('payload')
        throw new APIError('Use the SSO session endpoint; SSO cannot issue Payload tokens.', 403)
      }
    }],
    me: [({ args, user }) => {
      if (args.req.user?._strategy !== 'webdock-sso') return
      const authenticated = args.req.user as typeof args.req.user & { role: Identity['role']; _ssoExp: number }
      return { user: { ...user, role: authenticated.role }, exp: authenticated._ssoExp }
    }],
  }
  return { ...session, hooks,
    strategy: { name: 'webdock-sso', authenticate: ({ headers, payload }: { headers: Headers; payload?: Payload }) =>
      options.lookupUser || !payload ? Promise.resolve({ user: null }) : session.authenticate({ headers }, identity => findPayloadUser(payload, identity)),
    },
  }
}
