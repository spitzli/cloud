import { createPreferenceResponse } from '@spitzli/i18n/preference';
export async function POST(request: Request) {
  return createPreferenceResponse(request, { canonicalOrigin: process.env.SPITZLI_CONSOLE_ORIGIN || process.env.NEXT_PUBLIC_SERVER_URL || 'https://studio.webdock.dev' });
}
