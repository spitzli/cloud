import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { nativeSchema } from './native-schema'

const source = process.env.TABULARIUM_SOURCE_ROOT
if (!source) throw new Error('Set TABULARIUM_SOURCE_ROOT')
const { buildSchema } = await import(resolve(source, 'packages/manifest/src/schema.ts'))
const { ManifestSchema } = await import(resolve(source, 'packages/manifest/src/core.ts'))
const { validateManifest } = await import(resolve(source, 'packages/manifest/src/validate.ts'))
const { validateKindManifestSchema } = await import(resolve(source, 'packages/manifest/src/root-schema.ts'))
const delta = JSON.parse(readFileSync('tools/plugins/tabularium-kind.schema.json', 'utf8'))
const v1 = buildSchema({ coreSchema: ManifestSchema, kind: 'extension', kindOverrides: { extension: delta } })
delete v1.$schema; delete v1.$id; delete v1.title
v1.properties.kind = { const: 'extension' }; v1.required.push('kind')
const v2 = nativeSchema(v1)
const union = validateKindManifestSchema({ oneOf: [v1, v2] }, 'extension')
const specific = buildSchema({ coreSchema: ManifestSchema, kind: 'extension', kindSchemas: { extension: union } })
const global = buildSchema({ coreSchema: ManifestSchema, kindSchemas: { extension: union } })
const read = (path: string) => JSON.parse(readFileSync(path, 'utf8'))
const finalized = (name: string) => ({ ...read(`plugins/${name}/manifest.json`), image: `ghcr.io/spitzli/${name}@sha256:${'a'.repeat(64)}` })

test('complete native product manifests and existing v1 releases validate in both discovery forms', () => {
  for (const name of ['echo-demo', 'hosting', 'mail', 'archives', 'object-storage', 'postgres']) {
    for (const schema of [specific, global]) {
      const result = validateManifest(finalized(name), schema)
      if (!result.ok) console.error(name, result.errors.slice(0, 8))
      expect(result.ok).toBe(true)
    }
  }
  for (const name of ['project-notes', 'managed-analytics', 'managed-apps', 'lumaserv', 'managed-registry']) {
    const manifest = read(`../webdock-extension-${name}/tabularium.json`)
    expect(validateManifest(manifest, specific).ok).toBe(true)
    expect(validateManifest(manifest, global).ok).toBe(true)
  }
  if (v1.properties.id) {
    const legacyDisplay = { ...read('../webdock-extension-project-notes/tabularium.json'), id: 'project-notes', name: 'Project Notes' }
    expect(validateManifest(legacyDisplay, specific).ok).toBe(true)
    expect(validateManifest(legacyDisplay, global).ok).toBe(true)
  }
})

test('native root rejects mixed tuples, privilege expansion, mutable/zero images and excessive schema depth', () => {
  const fixture = finalized('hosting')
  for (const patch of [
    { id: 'native-id' }, { name: 'Human Native Display Name' },
    { protocolVersion: 1 }, { sdkVersion: 1 }, { dataProtocolVersion: 1 },
    { image: 'ghcr.io/spitzli/hosting:latest' }, { image: `ghcr.io/spitzli/hosting@sha256:${'0'.repeat(64)}` },
    { privileged: true }, { egress: [{ host: '*', port: 443 }] }, { capabilities: ['plugin.root'] },
    { services: { required: [{ name: 'root', version: 1 }], optional: [] } },
    { workloads: { kinds: ['v1/Node'] } }, { capabilities: [] },
    { configuration: { type: 'object', properties: Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`field${i}`, { type: 'string' }])) } },
    { actions: Array(65).fill(fixture.actions[0]) },
  ]) {
    expect(validateManifest({ ...fixture, ...patch }, specific).ok).toBe(false)
    expect(validateManifest({ ...fixture, ...patch }, global).ok).toBe(false)
  }
  let deep: any = { type: 'string' }
  for (let i = 0; i < 5; i++) deep = { type: 'object', properties: { child: deep } }
  expect(validateManifest({ ...fixture, configuration: deep }, specific).ok).toBe(false)
})
