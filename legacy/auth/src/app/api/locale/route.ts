import { createPreferenceResponse } from '@webdock/i18n/preference';
export const POST = (request: Request) => createPreferenceResponse(request, {
 canonicalOrigin: new URL(process.env.BETTER_AUTH_URL || 'http://localhost:3125').origin,
});
