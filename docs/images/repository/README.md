# Repository presentation assets

- `readme-banner.png`: 1600 × 440, used at the top of the root README.
- `social-preview.png`: 1280 × 640 PNG, configured as the repository's GitHub Social preview.
- Matching `.svg` files are editable sources. They embed the existing Spitzli logo unchanged;
  no external image fetch or new logo design is involved. Typography is Space Grotesk.
- `console-workspace.png`: a 1440 × 1000 screenshot of the real Console components at
  the workspace members page, with synthetic HTTP fixtures. Alex Demo, `alex@example.test`,
  Spitzli Demo and its Neon inventory are demo data. No live customer account, provider credential,
  connection URI or production API is used. It includes the workspace-icon feature in this branch.

To refresh the Console screenshot from the repository root:

```sh
bun apps/console/tests/repository-capture.mjs
```

The capture refuses external requests and supplies only synthetic customer API fixtures.
It renders the actual app; it does not replace or repaint its DOM.

The banner graphics use the existing Spitzli graphite/violet palette and the original
[`spitzli-logo.png`](../../../apps/console/public/spitzli-logo.png). PNG exports were rendered
from the SVG sources with `rsvg-convert` and the project's existing Space Grotesk font.
Install that font before re-rendering to preserve the typography; do not replace the logo.

```sh
rsvg-convert docs/images/repository/readme-banner.svg -o docs/images/repository/readme-banner.png
rsvg-convert docs/images/repository/social-preview.svg -o docs/images/repository/social-preview.png
```

GitHub Social preview is repository metadata, separate from a README image. Upload
`social-preview.png` under **Settings → General → Social preview**. The opaque PNG is below
GitHub's 1 MB limit; keep text clear of the edges. See
[GitHub's upload guidance](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/customizing-your-repositorys-social-media-preview).

Do not substitute screenshots of customer workspaces, provider keys, revealed database URIs,
operator-only information or private infrastructure details. Refresh screenshots from the actual
UI with visibly synthetic data when the layout changes; do not fabricate product screens.
