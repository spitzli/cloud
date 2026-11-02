/** Read-only cutover check; never rewrites exports or logs content/credentials.
 * bun --env-file=.env.local scripts/verify-instance-source.mjs
 */
import fs from 'node:fs'
import path from 'node:path'
import { parseEnv } from 'node:util'
import { isDeepStrictEqual } from 'node:util'
import { Pool } from 'pg'
import { getPayload } from 'payload'

const readOnlyURL = value => {
  if (!value) throw new Error('Database connection is required')
  const url = new URL(value)
  url.searchParams.set('options', `${url.searchParams.get('options') || ''} -c default_transaction_read_only=on`.trim())
  return url.toString()
}
for (const name of ['DATABASE_URL', 'DATABASE_URL_UNPOOLED']) process.env[name] = readOnlyURL(process.env[name])
const config = (await import('../src/payload.config.ts')).default
const payload = await getPayload({ config })
const ledger = JSON.parse(fs.readFileSync('.backups/instance-blob-copies.json', 'utf8'))
const instancePaths = {
  webdock: process.env.WEBDOCK_INSTANCE_PATH || '../website',
  spitzli: process.env.SPITZLI_INSTANCE_PATH || '../../../spitzli',
  stall: process.env.STALL_INSTANCE_PATH || '/home/newt/Projekte/Personal/spitzli-backup-roelfs-20261002/stall-eichenbruch',
}
const canonical = value => {
  if (Array.isArray(value)) return value.map(canonical)
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]))
  return value
}
const comparable = value => canonical(Array.isArray(value) ? [...value].sort((a, b) => String(a.id).localeCompare(String(b.id))) : value)
const collection = async (slug, site, draft = false) => (await payload.find({ collection: slug, where: { site: { equals: site } }, locale: 'all', draft, overrideAccess: true, depth: 0, pagination: false, showHiddenFields: true })).docs
const versions = async (slug, site) => (await payload.findVersions({ collection: slug, where: { 'version.site': { equals: site } }, locale: 'all', overrideAccess: true, depth: 0, pagination: false })).docs
const reports = []
let failed = false
try {
  const readOnly = (await payload.db.pool.query('SHOW default_transaction_read_only')).rows[0].default_transaction_read_only
  if (readOnly !== 'on') throw new Error('Source connection must be read-only')
  const sites = (await payload.find({ collection: 'sites', overrideAccess: true, depth: 0, pagination: false })).docs
  const accounts = (await payload.db.pool.query('SELECT id,name,email,hash,salt FROM public.users')).rows
  const memberships = (await payload.find({ collection: 'users', overrideAccess: true, depth: 0, pagination: false })).docs
  for (const site of sites) {
    const name = site.key === 'stall-eichenbruch' ? 'stall' : site.key
    if (!(name in instancePaths)) continue
    const snapshot = JSON.parse(fs.readFileSync(`.backups/${name}-current.json`, 'utf8'))
    const users = accounts.flatMap(account => {
      const user = memberships.find(item => item.id === account.id)
      if (!user) return []
      const membership = user.tenants?.find(item => Number(item.tenant) === Number(site.tenant))
      const role = user.role === 'super-admin' ? 'operator' : membership?.role === 'tenant-admin' ? 'admin' : membership?.role
      return role ? [{ ...account, role }] : []
    })
    let current = { users }
    if (name === 'webdock') current = { ...current, landing: (await collection('landing-pages', site.id))[0], versions: await versions('landing-pages', site.id) }
    if (name === 'spitzli') current = { ...current, settings: (await collection('website-settings', site.id))[0], clients: await collection('clients', site.id), media: await collection('media', site.id), projects: await collection('projects', site.id), drafts: await collection('projects', site.id, true), versions: await versions('projects', site.id) }
    if (name === 'stall') current = { ...current, settings: (await collection('stall-settings', site.id))[0], header: (await collection('stall-header', site.id))[0], footer: (await collection('stall-footer', site.id))[0], media: await collection('media', site.id), forms: await collection('stall-forms', site.id), submissions: await collection('stall-form-submissions', site.id), redirects: await collection('stall-redirects', site.id), pages: await collection('stall-pages', site.id), drafts: await collection('stall-pages', site.id, true), versions: await versions('stall-pages', site.id) }
    let uncopiedAssets = 0
    for (const media of current.media || []) {
      const thumbnail = media.thumbnailURL
      for (const asset of [media, ...Object.values(media.sizes || {})]) {
        if (!asset?.url || !asset.filename) continue
        const copied = ledger[`${name}|${asset.url}`]
        if (copied) asset.url = copied
        else uncopiedAssets++
      }
      if (thumbnail && ledger[`${name}|${thumbnail}`]) media.thumbnailURL = ledger[`${name}|${thumbnail}`]
      media.prefix = `instances/${name}`
      media._objectKey = null
    }
    const changedSections = Object.keys(current).filter(key => !isDeepStrictEqual(comparable(current[key]), comparable(snapshot[key])))
    const counts = Object.fromEntries(Object.entries(current).filter(([, value]) => Array.isArray(value)).map(([key, value]) => [key, value.length]))
    reports.push({ site: name, sourceDrift: changedSections.length > 0, changedSections, uncopiedAssets, counts })
    if (changedSections.length || uncopiedAssets) failed = true
  }
  for (const [schema, directory] of Object.entries(instancePaths)) {
    const env = parseEnv(fs.readFileSync(path.join(directory, '.env.instance'), 'utf8'))
    const pool = new Pool({ connectionString: readOnlyURL(env.DATABASE_URL_UNPOOLED || env.DATABASE_URL), max: 1 })
    try {
      const who = (await pool.query('SELECT current_user, current_schema()')).rows[0]
      if (who.current_user !== `${schema}_runtime` || who.current_schema !== schema) throw new Error('Unexpected instance database identity')
      await pool.query(`SELECT id FROM "${schema}".users LIMIT 1`)
      let denied = 0
      for (const other of ['public', ...Object.keys(instancePaths)].filter(name => name !== schema)) {
        try { await pool.query(`SELECT id FROM "${other}".users LIMIT 1`) }
        catch (error) { if (error.code === '42501') { denied++; continue } throw error }
        throw new Error('Unexpected cross-schema table access')
      }
      reports.push({ site: schema, ownUsersSelect: 'allowed', otherUsersSelect: 'denied', deniedSchemas: denied })
    } finally { await pool.end() }
  }
  console.log(JSON.stringify({ checkedAt: new Date().toISOString(), readOnly: true, passed: !failed, reports }, null, 2))
  if (failed) process.exitCode = 1
} catch (error) {
  console.error(JSON.stringify({ passed: false, errorType: error.code || error.name }))
  process.exitCode = 1
} finally { await payload.destroy() }
