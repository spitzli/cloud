import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

type Schema = Record<string, any>
const string = (maxLength: number, minLength = 1, pattern?: string): Schema => ({ type: 'string', minLength, maxLength, ...(pattern ? { pattern } : {}) })
const slug = () => string(64, 1, '^[a-z][a-z0-9-]*$')
const object = (properties: Schema, required = Object.keys(properties)): Schema => ({ type: 'object', properties, required, additionalProperties: false })
const array = (items: Schema, maxItems: number, minItems = 0): Schema => ({ type: 'array', items, maxItems, minItems })
const labelled = (optional: Schema = {}): Schema => object({ id: slug(), label: string(80), ...optional }, ['id', 'label'])
const unwrap = (value: any): any => {
  if (Array.isArray(value)) return value.map(unwrap)
  if (!value || typeof value !== 'object') return value
  const result = Object.fromEntries(Object.entries(value).map(([key, child]) => [key, unwrap(child)])) as Schema
  if (result.allOf?.length === 1) {
    const { allOf, ...outer } = result
    return { ...outer, ...allOf[0] }
  }
  return result
}

/** Structural v2 contract. Dynamic ID/reference semantics remain Core parser checks. */
export function nativeSchema(legacy: Schema, protocolSource = 'crates/plugin-protocol/src/lib.rs'): Schema {
  const source = readFileSync(protocolSource, 'utf8')
  const list = (name: string, text = source): string[] => {
    const body = text.match(new RegExp(`(?:pub )?const ${name}: &[\\s\\S]*?= &\\[([\\s\\S]*?)\\];`))?.[1]
    if (!body) throw new Error(`Missing authoritative ${name}`)
    return [...body.matchAll(/"([^"]+)"/g)].map(match => match[1])
  }
  const capabilities = list('CAPABILITIES'), readCapabilities = list('READ_CAPABILITIES'), workloadKinds = list('WORKLOAD_KINDS')
  const serviceNames = list('SERVICES', readFileSync(resolve(dirname(protocolSource), 'manifest.rs'), 'utf8'))
  const result = unwrap(structuredClone(legacy))
  delete result.$schema; delete result.$id; delete result.title
  const p = result.properties
  delete p.id
  delete p.navigation; delete p.slots
  p.kind = { const: 'extension' }
  p.name = { ...slug(), not: { enum: ['core', 'auth', 'registry', 'plugins', 'billing'] } }
  for (const key of ['protocolVersion', 'sdkVersion', 'dataProtocolVersion']) p[key] = { const: 2 }
  p.image = { ...p.image, not: { pattern: '@sha256:0{64}$' } }
  const caps = { ...array({ type: 'string', enum: capabilities }, 32), uniqueItems: true }
  p.capabilities = caps
  p.secrets.uniqueItems = true; p.httpBindings.uniqueItems = true
  for (const serviceList of ['required', 'optional']) {
    p.services.properties[serviceList].items.properties.name.enum = serviceNames
    p.services.properties[serviceList].uniqueItems = true
  }
  p.workloads = object({ kinds: { ...array({ type: 'string', enum: workloadKinds }, workloadKinds.length, 1), uniqueItems: true } })
  const action = p.actions.items
  action.properties = { ...action.properties, capabilities: caps, audience: { enum: ['operator', 'customer'] },
    offering: slug(), feature: slug(), resource: { enum: ['create', 'use', 'delete'] } }
  action.required.push('audience')
  action.allOf = [
    { if: { properties: { intent: { const: 'read' } }, required: ['intent'] }, then: { properties: { capabilities: array({ enum: readCapabilities }, 32) } } },
    { if: { properties: { audience: { const: 'operator' } }, required: ['audience'] }, then: { not: { anyOf: [{ required: ['feature'] }, { required: ['resource'] }] } },
      else: { anyOf: [{ required: ['feature'] }, { required: ['resource'] }] } },
    { if: { properties: { resource: { enum: ['create', 'delete'] } }, required: ['resource'] }, then: { properties: { intent: { const: 'write' } } } },
    { if: { properties: { resource: { const: 'create' } }, required: ['resource'] }, then: { required: ['offering', 'feature'],
      properties: { input: { properties: { properties: { not: { required: ['resource_id'] } } } } } } },
    { if: { properties: { resource: { enum: ['use', 'delete'] } }, required: ['resource'] }, then: { required: ['input'], properties: { input: {
      required: ['properties', 'required'], properties: { properties: { required: ['resource_id'], properties: { resource_id: {
        properties: { type: { const: 'string' }, maxLength: { maximum: 128 } }, required: ['type', 'maxLength'] } } }, required: { contains: { const: 'resource_id' } } } } } } },
    { if: { properties: { storage: { const: 'upload' } }, required: ['storage'] }, then: { properties: { intent: { const: 'write' }, capabilities: { contains: { const: 'plugin.storage.write' } } } } },
    { if: { properties: { storage: { const: 'download' } }, required: ['storage'] }, then: { properties: { capabilities: { contains: { const: 'plugin.storage.read' } } } } },
  ]
  p.jobs.items.properties.capabilities = caps
  p.offerings = array(object({ id: slug(), label: string(80), description: string(280), highlights: array(string(140), 6, 1) }, ['id', 'label', 'description']), 16)
  p.selfService = object({ features: array(labelled({ offering: slug(), description: string(280, 0) }), 32) })
  const page = (customer: boolean) => object({ id: slug(), label: string(80), action: string(64, 1, '^[a-z][a-z0-9_.-]*$'), ...(customer ? { resource: { const: true } } : {}) })
  p.areas = object({ operator: object({ pages: array(page(false), 16) }), selfService: object({ pages: array(page(true), 16) }) }, ['operator'])
  result.required = ['name', 'version', 'kind', 'protocolVersion', 'sdkVersion', 'dataProtocolVersion', 'coreVersion', 'image', 'dependencies', 'capabilities',
    'schemaVersion', 'readableSchema', 'migrations', 'tables', 'services', 'configuration', 'actions', 'jobs', 'offerings', 'selfService', 'areas', 'events', 'secrets', 'httpBindings']
  result.allOf = [
    { if: { properties: { capabilities: { contains: { const: 'plugin.workloads' } } }, required: ['capabilities'] }, then: { required: ['workloads'] }, else: { not: { required: ['workloads'] } } },
    { if: { properties: { actions: { contains: { properties: { audience: { const: 'customer' } }, required: ['audience'] } } } }, then: { properties: { areas: { required: ['selfService'] } } } },
    ...[0, 12].map(start => ({ allOf: capabilities.slice(start, start + 12).map(capability => ({
      if: { properties: { capabilities: { not: { contains: { const: capability } } } } },
      then: { properties: { actions: { items: { properties: { capabilities: { not: { contains: { const: capability } } } } } }, jobs: { items: { properties: { capabilities: { not: { contains: { const: capability } } } } } } } },
    })) })),
  ]
  // The v1 schema already bounds supported schema descriptions to three child
  // levels. Add the native 64-field map cap wherever those descriptors recur.
  const boundMaps = (node: any): void => {
    if (!node || typeof node !== 'object') return
    if (node.type === 'object' && node.propertyNames) node.maxProperties = 64
    for (const child of Object.values(node)) if (typeof child === 'object') boundMaps(child)
  }
  boundMaps(result)
  return result
}
