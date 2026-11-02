/* eslint-disable @typescript-eslint/no-explicit-any -- The archived Payload 3 JSON is recursively remapped before Payload 4 validates it. */
/** Run only after the schema migration and Stall tenant/site provisioning.
 * bun --env-file=.env.local scripts/import-stall.ts
 * Originals and the private export stay untouched. Never sends notifications.
 */
import { readFile, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { getPayload, type CollectionSlug, type JsonObject } from 'payload'
import config from '../src/payload.config'

type Doc = Record<string, any> // Source schema is the older Payload 3 snapshot.
const input = JSON.parse(await readFile('.migration-stall.json', 'utf8')) as Record<string, Doc[] | Doc>
const ledgerPath = '.migration-stall-state.json'
let ledger: { versions: string[] } = { versions: [] }
try { ledger = JSON.parse(await readFile(ledgerPath, 'utf8')) } catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e }
const saveLedger = () => writeFile(ledgerPath, JSON.stringify(ledger, null, 2), { mode: 0o600 })
const payload = await getPayload({ config })
const sites = await payload.find({ collection: 'sites', where: { key: { equals: 'stall-eichenbruch' } }, overrideAccess: true, depth: 0 })
const site = sites.docs[0]
if (!site) throw new Error('Provision stall-eichenbruch site first')
if ((input['form-submissions'] as Doc[]).length) throw new Error('Submission import requires a notification-free database import; refusing to trigger emails')
const scope = { site: site.id, tenant: typeof site.tenant === 'object' ? site.tenant?.id : site.tenant }
const maps: Record<string, Map<number, number>> = { media: new Map(), pages: new Map(), forms: new Map() }
const clean = (doc: Doc) => { const data = { ...doc }; for (const key of ['id', 'createdAt', 'updatedAt', 'usedInHero', 'usedInSeo', 'usedInBlocks']) delete data[key]; return data }
const rows = (key: string) => input[key] as Doc[]
const context = { migration: true, disableRevalidate: true }
const existing = async (collection: CollectionSlug, sourceID: string) => (await payload.find({ collection, where: { and: [{ site: { equals: site.id } }, { sourceID: { equals: sourceID } }] }, depth: 0, limit: 1, overrideAccess: true })).docs[0]
async function upsert(collection: CollectionSlug, sourceID: string, data: Doc) {
  const found = await existing(collection, sourceID)
  const args = { collection, data: { ...data, ...scope, sourceID } as never, overrideAccess: true, context, locale: 'de' as const }
  return found ? payload.update({ ...args, id: found.id }) : payload.create(args)
}
function remap(value: any, key = ''): any {
  if (value === null || value === undefined) return value
  if (Array.isArray(value)) return value.map(v => remap(v, key === 'images' ? 'media' : key))
  if (typeof value === 'number') {
    const kind = ['media','image','photo'].includes(key) ? 'media' : key === 'form' ? 'forms' : undefined
    if (!kind) return value
    const mapped = maps[kind].get(value)
    if (!mapped) throw new Error(`Unmapped ${kind} ID ${value}`)
    return mapped
  }
  if (typeof value !== 'object') return value
  if (value.relationTo && 'value' in value) {
    const kind = String(value.relationTo).replace(/^stall-/, '')
    const map = maps[kind]
    return { ...value, relationTo: kind === 'pages' || kind === 'forms' ? `stall-${kind}` : kind, value: map ? map.get(Number(typeof value.value === 'object' ? value.value.id : value.value)) : value.value }
  }
  return Object.fromEntries(Object.entries(value).map(([k,v]) => [k, remap(v,k)]))
}
try {
  for (const media of rows('media')) {
    const sourceID = String(media.id)
    let doc = await existing('media', sourceID)
    if (!doc) {
      const url = new URL(media.url || `/api/media/file/${encodeURIComponent(media.filename)}`, String(input.source))
      const response = await fetch(url)
      if (!response.ok) throw new Error(`Source media download failed: ${response.status}`)
      const buffer = Buffer.from(await response.arrayBuffer())
      const extension = String(media.filename).split('.').pop()
      doc = await payload.create({ collection: 'media', overrideAccess: true, context, locale: 'de', data: { ...scope, sourceID, alt: media.alt, caption: media.caption, focalX: media.focalX, focalY: media.focalY } as never, file: { data: buffer, name: `${randomUUID()}.${extension}`, mimetype: media.mimeType, size: buffer.length } })
    }
    await payload.update({ collection: 'media', id: doc.id, locale: 'en', data: { alt: media.alt }, overrideAccess: true, context })
    maps.media.set(media.id, Number(doc.id))
  }
  console.log(`Media ready: ${maps.media.size}`)
  for (const form of rows('forms')) {
    const doc = await upsert('stall-forms', String(form.id), clean(form))
    maps.forms.set(form.id, Number(doc.id))
  }
  // Allocate page IDs before resolving circular navigation/internal-page links.
  for (const page of rows('pages')) {
    const found = await existing('stall-pages', String(page.id))
    const doc = found || await upsert('stall-pages', String(page.id), { title: page.title, slug: page.slug, hero: { type: 'none' }, layout: [{ blockType: 'contact' }], _status: 'draft' })
    maps.pages.set(page.id, Number(doc.id))
  }
  for (const page of rows('pages')) await upsert('stall-pages', String(page.id), remap(clean(page)))
  for (const [source, collection] of [['site-info','stall-settings'],['header','stall-header'],['footer','stall-footer']] as const) {
    const data = remap(clean(input[source] as Doc))
    if (data.maintenance) delete data.maintenance.bypassKey
    await upsert(collection, source, data)
  }
  for (const redirect of rows('redirects')) await upsert('stall-redirects', String(redirect.id), remap(clean(redirect)))
  for (const version of [...rows('versions')].sort((a,b) => a.updatedAt.localeCompare(b.updatedAt))) {
    if (ledger.versions.includes(String(version.id))) continue
    const parent = maps.pages.get(Number(version.parent))
    if (!parent) throw new Error('Unknown version parent')
    const created = await payload.db.createVersion({ collectionSlug: 'stall-pages', parent, autosave: Boolean(version.autosave), createdAt: version.createdAt, updatedAt: version.updatedAt, versionData: { ...remap(clean(version.version)), ...scope, sourceID: String(version.parent) } as JsonObject })
    // Historical imports must not become the active draft snapshot.
    await payload.db.updateVersion({ collection: 'stall-pages', id: created.id, versionData: { latest: false, version: created.version, createdAt: version.createdAt, updatedAt: version.updatedAt } })
    ledger.versions.push(String(version.id)); await saveLedger()
  }
  for (const draft of rows('drafts') || []) {
    if (draft._status !== 'draft') continue
    await payload.update({ collection: 'stall-pages', id: maps.pages.get(draft.id)!, data: { ...remap(clean(draft)), ...scope, sourceID: String(draft.id) }, draft: true, overrideAccess: true, context })
  }
  console.log(JSON.stringify({ media: maps.media.size, pages: maps.pages.size, forms: maps.forms.size, historicalVersions: ledger.versions.length }))
} finally { await payload.destroy() }
