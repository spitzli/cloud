import { expect, test } from 'bun:test'
import { resolve } from 'node:path'
import { providerKind } from './provider-kind'
const source = process.env.TABULARIUM_SOURCE_ROOT
if (!source) throw new Error('Set TABULARIUM_SOURCE_ROOT')
const { validateKindManifestSchema } = await import(resolve(source, 'packages/manifest/src/root-schema.ts'))
const { validateManifest } = await import(resolve(source, 'packages/manifest/src/validate.ts'))
const { buildSchema } = await import(resolve(source, 'packages/manifest/src/schema.ts'))
const { ManifestSchema } = await import(resolve(source, 'packages/manifest/src/core.ts'))
const kind = providerKind()
const checked = validateKindManifestSchema(kind.manifestSchema, 'provider')
const schema = buildSchema({ coreSchema: ManifestSchema, kindSchemas: { provider: checked } })
const candidate = (repository: boolean) => ({ name: repository ? 'github-repository' : 'mail-relay', version: '0.1.0', kind: 'provider',
  description: 'Isolated test catalog metadata, never a published provider binding',
  image: `ghcr.io/spitzli/fixture@sha256:${'a'.repeat(64)}`,
  sourceRepo: 'https://github.com/spitzli/fixture', revision: 'b'.repeat(40), catalogOnly: true,
  protocol: repository ? 'webdock.repository.v1' : 'webdock.mail.v1',
  runtimeBinding: { status: 'required-before-activation', protocol: repository ? 'webdock.repository.v1' : 'webdock.mail.v1',
    authenticity: repository ? 'publisher-signed-descriptor' : 'core-descriptor-hash', genericExtensionInstaller: false },
})

test('pending catalog publication requires no generated descriptor or publisher key', () => {
  for (const repository of [true, false]) expect(validateManifest(candidate(repository), schema).ok).toBe(true)
})
test('protocol identity, immutable source and absence of generic install claims are strict', () => {
  const original = candidate(true)
  for (const patch of [ { kind: 'extension' }, { catalogOnly: false }, { protocol: 'webdock.plugin.v2' }, { protocolVersion: 2 },
    { image: 'ghcr.io/spitzli/fixture:latest' }, { image: `ghcr.io/spitzli/fixture@sha256:${'0'.repeat(64)}` },
    { revision: 'main' }, { sourceRepo: 'https://github.com/spitzli/fixture/tree/main' }, { privileged: true },
    { runtimeBinding: { ...original.runtimeBinding, authenticity: 'core-descriptor-hash' } },
    { runtimeBinding: { ...original.runtimeBinding, status: 'configured' } },
    { runtimeBinding: { ...original.runtimeBinding, descriptor: { sha256: 'c'.repeat(64) } } },
    { runtimeBinding: { ...original.runtimeBinding, genericExtensionInstaller: true } },
  ]) expect(validateManifest({ ...original, ...patch }, schema).ok).toBe(false)
})
test('configured repository signature binding and Mail Core hash cannot be interchanged', () => {
  const repository = candidate(true), mail = candidate(false)
  const repoDescriptor = { sha256: 'c'.repeat(64), publisherPublicKeySha256: 'd'.repeat(64), signerKeyID: 'fixture',
    domain: 'webdock.repository.descriptor.v1', boundManifestSha256: 'e'.repeat(64), imageBindingRequired: true }
  const mailDescriptor = { sha256: 'c'.repeat(64), authority: 'core', signedLeaseDescriptorMatchRequired: true }
  const configured = (value: any, descriptor: any) => ({ ...value, runtimeBinding: { ...value.runtimeBinding, status: 'configured', transport: 'unix-peer-uid', descriptor } })
  expect(validateManifest(configured(repository, repoDescriptor), schema).ok).toBe(true)
  expect(validateManifest(configured(mail, mailDescriptor), schema).ok).toBe(true)
  expect(validateManifest({ ...configured(repository, repoDescriptor), ...presentation }, schema).ok).toBe(true)
  expect(validateManifest({ ...configured(mail, mailDescriptor), ...presentation }, schema).ok).toBe(true)
  expect(validateManifest(configured(repository, mailDescriptor), schema).ok).toBe(false)
  expect(validateManifest(configured(mail, repoDescriptor), schema).ok).toBe(false)
})

const presentation = {
  icon: 'assets/icon.svg', readme: 'README.md',
  screenshots: [{ url: 'assets/screenshot.svg', caption: 'Provider setup', alt: 'Provider activation requires a Core binding' }],
}

test('both provider protocols accept optional locked-core presentation without requiring it', () => {
  for (const repository of [true, false]) {
    const original = candidate(repository)
    expect(validateManifest(original, schema).ok).toBe(true)
    expect(validateManifest({ ...original, ...presentation }, schema).ok).toBe(true)
    expect(validateManifest({ ...original, icon: '', readme: '', screenshots: [] }, schema).ok).toBe(true)
    expect(validateManifest({ ...original, icon: 'x'.repeat(500), readme: 'x'.repeat(500),
      screenshots: Array.from({ length: 12 }, () => ({ url: 'x'.repeat(500), caption: 'x'.repeat(200), alt: 'x'.repeat(200) })) }, schema).ok).toBe(true)
  }
})

test('presentation retains locked-core bounds and rejects unknown fields in both protocols', () => {
  for (const repository of [true, false]) {
    const original = { ...candidate(repository), ...presentation }
    for (const patch of [
      { icon: 'x'.repeat(501) }, { readme: 'x'.repeat(501) }, { screenshots: Array(13).fill({ url: 'a' }) },
      { screenshots: [{ url: '' }] }, { screenshots: [{ url: 'x'.repeat(501) }] },
      { screenshots: [{ url: 'a', caption: 'x'.repeat(201) }] }, { screenshots: [{ url: 'a', alt: 'x'.repeat(201) }] },
      { screenshots: [{ url: 'a', privileged: true }] }, { presentation: {} }, { privileged: true },
      { runtimeBinding: { ...original.runtimeBinding, descriptor: { sha256: 'c'.repeat(64) } } },
    ]) expect(validateManifest({ ...original, ...patch }, schema).ok).toBe(false)
  }
})

test('presentation field limits match the maintained registry locked core', () => {
  for (const branch of kind.manifestSchema.oneOf) {
    const core = ManifestSchema.properties
    expect(branch.properties.icon).toEqual({ type: 'string', maxLength: core.icon.maxLength })
    expect(branch.properties.readme).toEqual({ type: 'string', maxLength: core.readme.maxLength })
    const screenshots = branch.properties.screenshots as any
    expect(screenshots.maxItems).toBe(core.screenshots.maxItems)
    expect(screenshots.items.required).toEqual(core.screenshots.items.required)
    for (const field of ['url', 'caption', 'alt']) {
      const { type, minLength, maxLength } = core.screenshots.items.properties[field]
      expect(screenshots.items.properties[field]).toEqual({ type, ...(minLength === undefined ? {} : { minLength }), maxLength })
    }
    expect(branch.required).not.toContain('icon')
    expect(branch.required).not.toContain('readme')
    expect(branch.required).not.toContain('screenshots')
  }
})

test('generated provider schema artifact matches its maintained source', async () => {
  expect(await Bun.file(new URL('./provider-kind.json', import.meta.url)).json()).toEqual(kind)
})
