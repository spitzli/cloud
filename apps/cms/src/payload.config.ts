import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import { lexicalEditor } from '@payloadcms/richtext-lexical';
import { formBuilderPlugin } from '@payloadcms/plugin-form-builder';
import { vercelBlobStorage } from '@payloadcms/storage-vercel-blob';
import sharp from 'sharp';
import { Sites, Integrations } from './cms/platform';
import { Media } from './cms/media';
import { Clients, Projects } from './modules/spitzli/collections';
import { WebsiteSettings } from './modules/spitzli/settings';
import { stallCollections, stallFormOverrides, stallSubmissionOverrides } from './modules/stall';
import { modelScope } from './cms/collection-scope';
import { FormLimits, actionEndpoints } from './api/form-action';
import { contentEndpoints } from './api/content';
import { systemAccess } from './cms/system-access';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildConfig } from 'payload';
import { postgresAdapter } from '@payloadcms/db-postgres';
import nodemailer from 'nodemailer';
import { nodemailerAdapter } from '@payloadcms/email-nodemailer';
import { LandingPage } from './cms/landing';
import { editor, isSuperAdmin, selfOrSuperAdmin, signedIn, superAdmin, updateOwnAccount } from './cms/access';
import { multiTenantPlugin } from '@payloadcms/plugin-multi-tenant';

const databaseURL = process.env.PAYLOAD_MIGRATING === "true" ? process.env.DATABASE_URL_UNPOOLED : process.env.DATABASE_URL;
const databaseConnection = process.env.DATABASE_CA_CERT && databaseURL ? verifiedPostgresConfig(databaseURL, process.env.DATABASE_CA_CERT) : {connectionString: databaseURL};
const dirname = path.dirname(fileURLToPath(import.meta.url));
if (!process.env.PAYLOAD_SECRET || process.env.PAYLOAD_SECRET.length < 32) throw new Error('PAYLOAD_SECRET must have at least 32 characters.');
if (!process.env.BLOB_READ_WRITE_TOKEN) throw new Error('BLOB_READ_WRITE_TOKEN is required for persistent media storage.');
const serverURL = process.env.NEXT_PUBLIC_SERVER_URL || 'http://localhost:3105';

export default buildConfig({
  secret: process.env.PAYLOAD_SECRET || '',
  serverURL,
  baseAccess: systemAccess,
  jobs: { access: { run: superAdmin, queue: superAdmin, cancel: superAdmin } },
  editor: lexicalEditor(),
  localization: { locales: ['en','de'], defaultLocale: 'en', fallback: true },
  endpoints: [...contentEndpoints, ...actionEndpoints],
  admin: { components: { beforeNav: ['/components/SiteSelector#SiteSelector'] }, user: 'users', importMap: { baseDir: dirname }, meta: { titleSuffix: '— Webdock CMS' } },
  collections: [{
    slug: 'users',
    admin: { useAsTitle: 'email' },
    auth: { useAPIKey: true, maxLoginAttempts: 5, lockTime: 600000 },
    access: { admin: editor, create: superAdmin, read: selfOrSuperAdmin, update: updateOwnAccount, delete: superAdmin, readVersions: superAdmin, unlock: superAdmin },
    fields: [{ name: 'name', type: 'text' }, { name: 'role', type: 'select', required: true, defaultValue: 'member', saveToJWT: true, options: ['super-admin', 'member', 'editor', 'site-reader'], access: { create: superAdmin, update: superAdmin } }],
  }, {
    slug: 'tenants', admin: { useAsTitle: 'name' },
    access: { read: signedIn, create: superAdmin, update: superAdmin, delete: () => false },
    fields: [{ name: 'name', type: 'text', required: true }, { name: 'slug', type: 'text', required: true, unique: true }, { name: 'domain', type: 'text', required: true }],
  }, Sites, Integrations, FormLimits, modelScope(LandingPage,'pages','landing'), modelScope(Clients,'projects','portfolio'), modelScope(Projects,'projects','portfolio'), modelScope(WebsiteSettings,'site-settings','portfolio'), modelScope(Media,'media'), ...stallCollections.map(c => modelScope(c, c.slug==='stall-pages'?'pages':c.slug==='stall-redirects'?'redirects':'site-settings','business'))],
  plugins: [
    formBuilderPlugin({ fields: { payment: false, upload: false }, formOverrides: stallFormOverrides, formSubmissionOverrides: stallSubmissionOverrides, beforeEmail: () => [] }),
    (config) => ({...config, collections: config.collections?.map(collection => ['stall-forms','stall-form-submissions'].includes(collection.slug) ? modelScope(collection,'forms','business') : collection)}),
    multiTenantPlugin({
      collections: { sites: {useBaseFilter: false, tenantFieldOverrides:{access:{update: superAdmin}}}, 'landing-pages': {useBaseFilter: false}, clients: {useBaseFilter: false}, projects: {useBaseFilter: false}, media: {useBaseFilter: false}, 'website-settings': {useBaseFilter: false}, 'stall-pages': {useBaseFilter: false}, 'stall-settings': {useBaseFilter: false}, 'stall-header': {useBaseFilter: false}, 'stall-footer': {useBaseFilter: false}, 'stall-redirects': {useBaseFilter: false}, 'stall-forms': {useBaseFilter: false}, 'stall-form-submissions': {useBaseFilter: false} },
      tenantsArrayField: { rowFields: [{name:'role',type:'select',required:true,defaultValue:'editor',options:['tenant-admin','editor','reader']}] },
      useUsersTenantFilter: false,
      useTenantsListFilter: false,
      userHasAccessToAllTenants: isSuperAdmin, cleanupAfterTenantDelete: false,
    }),
    (config) => {
      const components=config.admin?.components;
      if(components?.beforeNav)components.beforeNav=components.beforeNav.filter(component=>!(typeof component==='object'&&component&&'path' in component&&component.path==='@payloadcms/plugin-multi-tenant/rsc#TenantSelector'));
      return config;
    },
  ],
  storage: [vercelBlobStorage({ token: process.env.BLOB_READ_WRITE_TOKEN, addRandomSuffix: false, collections: { media: { disablePayloadAccessControl: true, prefix: 'cms' } } })],
  sharp,
  upload: { limits: { fileSize: 10 * 1024 * 1024 }, abortOnLimit: true },
  db: postgresAdapter({
    pool: { ...databaseConnection, max: 3 },
    push: false,
    migrationDir: path.resolve(dirname, 'migrations'),
  }),
  email: nodemailerAdapter({
    defaultFromAddress: process.env.SMTP_FROM || 'noreply@webdock.dev',
    defaultFromName: 'Webdock',
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
