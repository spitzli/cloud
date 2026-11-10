#!/bin/sh
# Run ONLY inside an offline administrator-prepared VM, before converting to raw.
set -eu
[ "$(id -u)" = 0 ]
[ "$(node --version | cut -d. -f1)" = v24 ]
[ "$(vercel --version 2>/dev/null)" = 63.1.2 ]
node - "$(command -v vercel)" <<'NODE'
const fs = require('node:fs');
const assert = require('node:assert/strict');
const utils = require(require.resolve('@vercel/build-utils', { paths: [fs.realpathSync(process.argv[2])] }));
delete process.env.VERCEL_ALLOW_NODEJS_24;
utils.getSupportedNodeVersion('24.x').then(version => {
  assert.equal(version.major, 24);
  assert.equal(version.runtime, 'nodejs24.x');
}).catch(error => { console.error(error.message); process.exitCode = 1; });
NODE
test -x /usr/local/bin/buildctl
test -x /usr/local/bin/buildkitd
install -d -m 755 /opt/webdock-build-worker
install -m 644 worker.py guest.py /opt/webdock-build-worker/
install -m 644 spitzli-build-guest.service /etc/systemd/system/
cat > /etc/systemd/system/buildkit.service <<'EOF'
[Unit]
Description=Disposable guest BuildKit
[Service]
Environment=HTTP_PROXY=http://10.0.2.100:3128
Environment=HTTPS_PROXY=http://10.0.2.100:3128
Environment=NO_PROXY=localhost,127.0.0.1
ExecStart=/usr/local/bin/buildkitd --oci-worker=true --containerd-worker=false
[Install]
WantedBy=multi-user.target
EOF
printf 'virtio_blk\nvirtio_console\n' > /etc/modules-load.d/spitzli-build.conf
systemctl enable spitzli-build-guest.service buildkit.service
# The image must have no SSH/agent/cloud credentials, provider tokens, or source checkout.
