import path from 'node:path';
import type { NextConfig } from 'next';
import { withPayload } from '@payloadcms/next/withPayload';
const nextConfig: NextConfig = { experimental:{globalNotFound:true}, transpilePackages:['@spitzli/i18n','@spitzli/payload-sso','@spitzli/instance-kit','@spitzli/cms-ui', '@spitzli/cms-bridge'],outputFileTracingRoot:path.resolve(process.cwd(),'../..'),turbopack: { root: path.resolve(process.cwd(), '../..') } };
export default withPayload(nextConfig);
