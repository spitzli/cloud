# @spitzli/cli

Local TypeScript extension authoring tools for Spitzli Cloud, using protocol v1.
Requires Bun 1.4.3 or newer on `PATH`; npm can install the package.

Native Rust plugins use the separate [cloud-plugin-sdk](https://github.com/spitzli/cloud-plugin-sdk)
and its versioned runtime contracts.

```sh
npm install --global @spitzli/cli
spitzli-cloud plugin create my-extension --template actions --dir ./my-extension
spitzli-cloud plugin validate --dir ./my-extension
```

The `webdock` executable remains a compatibility alias for the same CLI.
The actions template vendors the installed, compiled `@spitzli/plugin-sdk` along
with its license and notice. The backend template depends on that SDK's exact
version. Commit the generated lockfile and keep dependencies pinned.

`create`, `validate`, `build` and `pack` operate locally. This CLI does not implement
registry publication, installation, deployment or a live development server.
Packing a descriptor is not proof that its referenced image has been published
or approved for installation. Existing protocol names and descriptor filenames
remain compatible.

Licensed under Apache-2.0; see LICENSE and NOTICE.
