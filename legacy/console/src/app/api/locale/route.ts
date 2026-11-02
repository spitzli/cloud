import { createPreferenceResponse } from '@webdock/i18n/preference';
export async function POST(request: Request) {
  return createPreferenceResponse(request, { canonicalOrigin: process.env.WEBDOCK_CONSOLE_ORIGIN || process.env.NEXT_PUBLIC_SERVER_URL || 'https://studio.webdock.dev' });
}
