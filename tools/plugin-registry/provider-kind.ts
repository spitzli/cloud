/** Catalog metadata only. Runtime admission remains the protocol-specific Core lane. */
export function providerKind() {
  const str = (maxLength: number, pattern?: string) => ({ type: 'string', minLength: 1, maxLength, ...(pattern ? { pattern } : {}) })
  const hash = { ...str(64, '^[a-f0-9]{64}$'), not: { const: '0'.repeat(64) } }
  const object = (properties: Record<string, unknown>, required = Object.keys(properties)) => ({ type: 'object', additionalProperties: false, properties, required })
  const image = { ...str(500, '^[a-z0-9]+(?:[.-][a-z0-9]+)*(?::[0-9]{1,5})?/(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*@sha256:[a-f0-9]{64}$'), not: { pattern: '@sha256:0{64}$' } }
  // Optional locked-core presentation limits: tabularium/packages/manifest/src/core.ts.
  const presentation = {
    icon: { type: 'string', maxLength: 500 },
    readme: { type: 'string', maxLength: 500 },
    screenshots: { type: 'array', maxItems: 12, items: object({
      url: str(500), caption: { type: 'string', maxLength: 200 }, alt: { type: 'string', maxLength: 200 },
    }, ['url']) },
  }
  const branch = (repository: boolean) => {
    const protocol = repository ? 'webdock.repository.v1' : 'webdock.mail.v1'
    const authenticity = repository ? 'publisher-signed-descriptor' : 'core-descriptor-hash'
    const descriptor = repository
      ? object({ sha256: hash, publisherPublicKeySha256: hash, signerKeyID: str(64, '^[a-z][a-z0-9-]*$'),
          domain: { const: 'webdock.repository.descriptor.v1' },
          boundManifestSha256: hash, imageBindingRequired: { const: true } })
      : object({ sha256: hash, authority: { const: 'core' }, signedLeaseDescriptorMatchRequired: { const: true } })
    const schema = object({
      name: str(64, '^[a-z][a-z0-9-]*$'), version: str(40, '^[0-9]+\\.[0-9]+\\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\\+[0-9A-Za-z.-]+)?$'),
      kind: { const: 'provider' }, description: str(280), image,
      sourceRepo: str(500, '^https://github\\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'),
      revision: str(40, '^[a-f0-9]{40}$'), protocol: { const: protocol }, catalogOnly: { const: true },
      runtimeBinding: {
        ...object({ status: { enum: ['required-before-activation', 'configured'] },
          protocol: { const: protocol }, authenticity: { const: authenticity },
          genericExtensionInstaller: { const: false },
          transport: { enum: ['unix-peer-uid', 'mutual-tls-leaf-pin'] }, descriptor,
        }, ['status', 'protocol', 'authenticity', 'genericExtensionInstaller']),
        allOf: [{ if: { properties: { status: { const: 'configured' } }, required: ['status'] },
          then: { required: ['descriptor', 'transport'] },
          else: { not: { anyOf: [{ required: ['descriptor'] }, { required: ['transport'] }] } } }],
      },
    })
    return { ...schema, properties: { ...schema.properties, ...presentation } }
  }
  return {
    key: 'provider', label: 'Protocol providers',
    description: 'Catalog-only provider artifacts. Activation requires explicit Core protocol binding and descriptor authentication; the generic extension installer cannot install these entries.',
    publicPageEnabled: true,
    manifestSchema: { oneOf: [branch(true), branch(false)] },
  }
}
if (import.meta.main) console.log(JSON.stringify(providerKind(), null, 2))
