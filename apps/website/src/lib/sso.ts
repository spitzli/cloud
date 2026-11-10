import { getPayload } from 'payload';
import { configurePayloadSSO } from '@spitzli/payload-sso';

export const sso: ReturnType<typeof configurePayloadSSO> | null = process.env.SPITZLI_SSO_CLIENT_ID
  ? configurePayloadSSO({
      getPayload: async () => getPayload({ config: (await import('../payload.config')).default }),
      successPath: '/cms',
      logoutPath: '/admin/login',
    })
  : null;
