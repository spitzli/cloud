#!/usr/bin/env python3
"""Extract explicit translation calls only from known Webdock source roots."""
from pathlib import Path
import subprocess
import sys

REPO = Path(__file__).resolve().parents[3]
OUTPUT = REPO / 'packages/i18n/locales/webdock.pot'
ROOTS = [REPO / path for path in (
    'legacy/auth/src', 'legacy/console/src', 'apps/console/src', 'apps/website/src', 'apps/cms-instance/src', 'apps/cms/src', 'packages/i18n/src',
    'packages/cms-ui/src', 'packages/instance-kit/src', 'packages/payload-sso/src',
    'packages/page-builder/src', 'packages/hosting-contracts/src', 'packages/search/src',
    'packages/console-kit/src', 'modules/mail/src', 'modules/hosting/src', 'modules/databases/src',
    'modules/cms/src', 'modules/storage/src',
)]
selected = [Path(path).resolve() for path in sys.argv[1:]] if len(sys.argv) > 1 else ROOTS
files = []
for root in selected:
    if not root.is_relative_to(REPO) or not any(root == allowed or root.is_relative_to(allowed) for allowed in ROOTS):
        raise SystemExit('Extraction paths must stay inside an explicit Webdock source root.')
    candidates = [root] if root.is_file() else root.rglob('*')
    for path in candidates:
        if path.suffix not in ('.ts', '.tsx', '.js', '.jsx', '.mjs') or path.name == 'payload-types.ts' or any(part.startswith('migrations') for part in path.parts) or '.test.' in path.name:
            continue
        if not path.resolve().is_relative_to(REPO):
            raise SystemExit('Source symlinks must stay inside the workspace.')
        files.append(str(path.relative_to(REPO)))
if not files:
    raise SystemExit('No source files selected.')
subprocess.run([
    'xgettext', '--from-code=UTF-8', '--keyword', '--keyword=t:1', '--keyword=n:1,2',
    '--keyword=p:1c,2', '--keyword=msgid:1', '--keyword=gettext:2', '--keyword=ngettext:2,3',
    '--keyword=pgettext:2c,3', '--add-comments=TRANSLATORS', '--no-wrap',
    '--package-name=Webdock', '--copyright-holder=Webdock', '--output=' + str(OUTPUT),
    *sorted(set(files)),
], cwd=REPO, check=True)
print(f'Extracted {len(set(files))} source files into {OUTPUT.relative_to(REPO)}.')
