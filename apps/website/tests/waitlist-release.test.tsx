import { afterAll, expect, mock, test } from 'bun:test';
import { createElement, Fragment } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { landingDefaults } from '../src/cms/landing';

const t = (message: string, values?: Record<string, string>) => message.replace(/\{(\w+)\}/g, (_, key: string) => values?.[key] ?? `{${key}}`);
mock.module('@webdock/i18n/next', () => ({ getRequestI18n: async () => ({ t, locale: 'en', preference: 'en', date: (value: string) => value }) }));
mock.module('@webdock/i18n/react', () => ({ I18nProvider: ({ children }: { children: React.ReactNode }) => createElement(Fragment, null, children) }));
mock.module('@webdock/i18n/picker', () => ({ LanguagePicker: () => createElement('span', null, 'Language') }));
mock.module('@/lib/landing', () => ({ getLanding: async () => ({ ...landingDefaults, heroButton: 'Email our team', useCases: landingDefaults.useCases.map((item, index) => ({ ...item, id: `fixture-${index}` })) }) }));
mock.module('@/components/dock', () => ({ Dock: () => createElement('div', null, 'Workspace preview') }));
mock.module('@/components/theme-picker', () => ({ ThemePicker: () => createElement('span', null, 'Theme') }));
mock.module('next/image', () => ({ default: (props: Record<string, unknown>) => createElement('img', props) }));
mock.module('next/script', () => ({ default: (props: Record<string, unknown>) => createElement('script', props) }));
mock.module('next/font/local', () => ({ default: () => ({ variable: 'local-font' }) }));

const { default: Home } = await import('../src/app/(frontend)/page');
const { default: Layout } = await import('../src/app/(frontend)/layout');
const { default: Privacy } = await import('../src/app/(frontend)/(legal)/privacy/page');
afterAll(() => mock.restore());

test('public entry requests waitlist access while retaining the authored contact action', async () => {
  const html = renderToStaticMarkup(await Home());
  const actions = html.match(/class="hero-actions[\s\S]*?<\/div>/)?.[0];
  expect(actions).toMatch(/class="button" href="https:\/\/console\.spitzli\.cloud\/waitlist">Join the waitlist/);
  expect(actions).toMatch(/href="mailto:[^"]+">Email our team/);
  expect(html).toMatch(/href="https:\/\/console\.spitzli\.cloud">Login/);
  expect(html).not.toContain('console.spitzli.cloud/login');
  expect(html).toContain('Access by invitation only.');
  expect(html).toContain('By invitation');
  expect(html).not.toContain('>Available<');
});

test('the production waitlist website does not load the retired analytics tracker', async () => {
  const before = process.env.VERCEL_ENV;
  process.env.VERCEL_ENV = 'production';
  try {
    const html = renderToStaticMarkup(await Layout({ children: createElement('main', null, 'Waitlist') }));
    expect(html).not.toContain('managed-analytics');
    expect(html).not.toContain('moon.webdock.dev');
  } finally {
    if (before === undefined) delete process.env.VERCEL_ENV;
    else process.env.VERCEL_ENV = before;
  }
});

test('privacy describes current processors and contact-only waitlist intake', async () => {
  const html = renderToStaticMarkup(await Privacy());
  expect(html).toContain('Neon for the platform database and managed sign-in');
  expect(html).not.toContain('Neon was the previous');
  expect(html).toContain('Saving a waitlist request does not verify your email, create an account or grant access.');
  expect(html).toContain('not to subscribe you to a marketing newsletter');
  expect(html).toContain('Optional website analytics are disabled');
  expect(html).not.toContain('We use self-hosted Umami');
});
