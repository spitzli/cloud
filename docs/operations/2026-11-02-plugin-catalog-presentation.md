# Plugin catalog presentation repair — 2026-11-02

The prepared presentation existed in independent Spitzli repositories but had not
been refreshed into the public catalog. Source publication alone had therefore
left ten native entries without their icons, rendered README or galleries.

Nine retained entries were refreshed through the normal authenticated registry
admin API at exact reviewed Git commits. Every source differed from its previous
release manifest only in presentation fields. Public image and Markdown URLs were
loaded and the Hosting detail page was rendered with loaded images. The signed
runtime manifest and asset identities were preserved. Integrity responses issue a
fresh `issued_at` claim when read; that timestamp is not an image/release change.

| Plugin | Source commit | Gallery images |
| --- | --- | --- |
| archives | `6ee4cc749e766e291ecaa6da0af64e13257a8a38` | 1 |
| echo-demo | `beca51cdf92b2def461aed669bfa6909d563489d` | 1 |
| github-repository | `5c14814e5195ceb3c15298546d5d2d27e0687d8d` | 1 |
| hosting | `aedc0434cc4cf9eedcffa129e69abac6cb463fab` | 1 |
| lumaserv-compute | `4c1386352039109daa210dfe6c237f7bf3d58c76` | 1 |
| mail | `21fc7c7ef4e3e9b9227dc6e752c96451d264f406` | 1 |
| mail-relay | `b649e84f98191c7e957957260fb209e66ce3e647` | 1 |
| managed-cms | `e82fe5586658c74ebc08fbd724be18723f315303` | 2 |
| object-storage | `11f5215d18842d904a5e4e69c9a3bacc25708348` | 1 |

The owner explicitly selected removal of the four legacy entries `lumaserv`,
`managed-analytics`, `managed-apps`, `project-notes`, plus `postgres`. Catalog and
release integrity snapshots were saved before their normal admin deletion.
Only registry entries/release rows were removed. Source repositories, image
artifacts, existing installations, customer data and volumes were not modified.
Legacy Analytics has existing website usage; catalog withdrawal is not service
uninstallation or an assertion of a replacement for every old capability.

Verification completed: nine public README/gallery/icon records, 28 public asset
and Markdown loads, 26 Ed25519 integrity signatures across saved/current records,
and five selected entries absent from the catalog. The temporary admin API token
used for exact-reference maintenance was revoked, rejection checked (401), and
its plaintext removed. The authenticated browser check tab was closed.

Use the Spitzli registry connector (`webdock_registry`) or the explicit
`https://registry.spitzli.cloud` API. A separate `tabularium` connector can refer
to a different instance; verify the instance and repository set before mutations.

New acceptance helper: `tools/plugin-release/catalog_presentation.py`. It checks
actual public presentation, not local file existence. Seventeen release-helper
unit tests pass, including the new missing/incorrect/mutable/inaccessible asset
cases. The current registry direct-publish path does not ingest README bytes;
normal exact-source refresh remains part of publication acceptance.

This presentation repair does not claim newly built Compute/CMS images, public
untrusted build readiness, or activation of uninstalled runtimes. The separate
Neon database plugin is tracked by its implementation plan and release evidence.
