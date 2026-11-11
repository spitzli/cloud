import type { NextConfig } from 'next';
import { withPayload } from '@payloadcms/next/withPayload';
const nextConfig: NextConfig = {
  experimental: { globalNotFound: true },
  transpilePackages: ['@payloadcms/next', '@payloadcms/ui', '@spitzli/i18n', '@spitzli/payload-sso', '@spitzli/instance-kit', '@spitzli/cms-ui', '@spitzli/cms-bridge'],
  // Keep Payload's error classes and database adapter in one native module graph.
  serverExternalPackages: ['payload', '@payloadcms/db-postgres', '@payloadcms/email-nodemailer', 'pg'],
};
export default withPayload(nextConfig);
