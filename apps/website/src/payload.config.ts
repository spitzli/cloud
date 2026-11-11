import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildConfig, type AuthStrategy } from 'payload';
import { postgresAdapter } from '@payloadcms/db-postgres';
import nodemailer from 'nodemailer';
import { nodemailerAdapter } from '@payloadcms/email-nodemailer';
import { authSubjectField } from '@spitzli/payload-sso';
import { sso } from './lib/sso';
export { sso } from './lib/sso';
import { LandingPage } from './cms/landing';
import { protectUsers, protectContent, isOperator } from './lib/instance-users';

const databaseURL = process.env.PAYLOAD_MIGRATING === "true" ? process.env.DATABASE_URL_UNPOOLED : process.env.DATABASE_URL;
const databaseConnection = process.env.DATABASE_CA_CERT && databaseURL ? verifiedPostgresConfig(databaseURL, process.env.DATABASE_CA_CERT) : {connectionString: databaseURL};
const dirname = path.dirname(fileURLToPath(import.meta.url));
const serverURL = process.env.NEXT_PUBLIC_SERVER_URL || 'http://localhost:3114';

export default buildConfig({
  secret: process.env.PAYLOAD_SECRET || '',
  serverURL,
  routes: { admin: "/system" },
  admin: { avatar: 'default', user: 'users', importMap: { baseDir: dirname, importMapFile: path.resolve(dirname, "app/(payload)/system/importMap.js") }, components: { beforeLogin: ['/components/sso-login#SSOLogin'] } },
  collections: [protectUsers({
    slug: 'users',
  admin: { useAsTitle: 'email' },
    auth: { maxLoginAttempts: 5, lockTime: 600000, strategies: sso ? [sso.strategy as AuthStrategy] : [], disableLocalStrategy: process.env.SPITZLI_SSO_ENFORCE === 'true' ? { enableFields: true, optionalPassword: true } : undefined },
    hooks: sso?.hooks,
    access: {},
    fields: [authSubjectField, { name: 'name', type: 'text' }, { name: 'disabled', type: 'checkbox', defaultValue: false, access: { create: isOperator, update: isOperator } }],
  }, process.env.OPERATOR_EMAIL || 'dominik@spitzli.dev')],
  globals: [protectContent(LandingPage)],
  jobs: { access: { run: isOperator, queue: isOperator, cancel: isOperator } },
  db: postgresAdapter({
    schemaName: 'webdock', disableCreateDatabase: true, allowIDOnCreate: true,
    pool: { ...databaseConnection, max: 3 },
    push: false,
    migrationDir: path.resolve(dirname, 'migrations-instance'),
  }),
  email: nodemailerAdapter({
    defaultFromAddress: process.env.SMTP_FROM || 'noreply@webdock.dev',
    defaultFromName: 'Spitzli Cloud',
    skipVerify: true,
    transport: nodemailer.createTransport({
      host: process.env.SMTP_HOST,
      port: Number(process.env.SMTP_PORT || 465),
      secure: true,
      auth: { user: process.env.SMTP_USER, pass: process.env.SMTP_PASS },
    }),
  }),
  typescript: { outputFile: path.resolve(dirname, 'payload-types.ts') },
  graphQL: { disable: true },
});
