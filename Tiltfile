# Local development: PostgreSQL in Docker, native Core and Console as host processes.
# Start with `tilt up`; generated loopback credentials live in .dev-local/ (gitignored).
# Requires Docker, the pinned Rust toolchain and Node 24 on PATH. `tilt down` keeps the database volume;
# `docker compose -f deploy/dev/compose.yaml down -v` resets it.

local('bun tools/dev/local.mjs secrets', quiet=True)
local('bun tools/dev/minio.mjs', quiet=True)

# WEBDOCK_DEV_HTTPS=1 tilt up: Console and account pages over HTTPS (needs mkcert; reset when switching).
https = os.getenv('WEBDOCK_DEV_HTTPS', '') == '1'
scheme = 'https' if https else 'http'
tls_env = {
    'WEBDOCK_DEV_TLS_CERT': os.path.join(config.main_dir, '.dev-local/tls/cert.pem'),
    'WEBDOCK_DEV_TLS_KEY': os.path.join(config.main_dir, '.dev-local/tls/key.pem'),
} if https else {}

docker_compose('deploy/dev/compose.yaml', project_name='webdock-dev')
dc_resource('postgres', labels=['infra'])
dc_resource('registry', labels=['infra'], links=[link('http://127.0.0.1:5005/v2/_catalog', 'catalog')])
dc_resource('mailpit', labels=['infra'], links=[link('http://127.0.0.1:55026', 'Mailpit')])
dc_resource('shared-postgres', labels=['infra'])
dc_resource('minio', labels=['infra'], links=[
    link('http://127.0.0.1:' + os.getenv('WEBDOCK_DEV_S3_PORT', '55900'), 'S3'),
    link('http://127.0.0.1:' + os.getenv('WEBDOCK_DEV_MINIO_CONSOLE_PORT', '55901'), 'MinIO'),
])
dc_resource('k3s', labels=['infra'])
local_resource(
    'kubeconfig',
    cmd='bun tools/dev/local.mjs kubeconfig',
    deps=['tools/dev/local.mjs'],
    resource_deps=['k3s'],
    labels=['infra'],
)
local_resource(
    'cluster-ca',
    cmd='bun tools/dev/local.mjs cluster-ca',
    deps=['tools/dev/local.mjs'],
    resource_deps=['kubeconfig'],
    labels=['infra'],
)

local_resource(
    'core-build',
    cmd='cargo build -p webdock-control-plane --bins',
    deps=['crates', 'Cargo.toml', 'Cargo.lock', 'proto'],
    ignore=['**/target', 'crates/**/tests'],
    labels=['core'],
)

# Offline migrations and the one-time operator bootstrap; reruns when migrations change.
local_resource(
    'database',
    cmd='bun tools/dev/local.mjs init',
    deps=['crates/persistence/migrations', 'crates/persistence/schema', 'crates/access/migrations', 'crates/registry/migrations', 'tools/dev/local.mjs'],
    resource_deps=['postgres', 'core-build'],
    labels=['infra'],
)

# Plugins: each image is pushed to the local registry once and recorded as an installed release;
# the binary runs natively from the plugin's own workspace over private Unix sockets.
plugins = {
    'echo-demo': {'binary': 'webdock-echo-demo', 'deps': []},
    'postgres': {'binary': 'webdock-postgres', 'deps': ['shared-postgres']},
    'mail': {'binary': 'webdock-mail', 'deps': ['mailpit']},
    'hosting': {'binary': 'webdock-hosting', 'deps': []},
}
for name in plugins:
    local_resource(
        name + '-image',
        cmd='bun tools/dev/local.mjs plugin-image ' + name,
        resource_deps=['registry'],
        labels=['plugins'],
    )

local_resource(
    'seed',
    cmd='bun tools/dev/local.mjs seed',
    deps=['tools/dev/local.mjs'] + ['plugins/%s/manifest.json' % name for name in plugins],
    resource_deps=['database', 'kubeconfig'] + [name + '-image' for name in plugins],
    labels=['infra'],
)

local_resource(
    'plugin-control',
    serve_cmd="sh -c 'set -a && . .dev-local/runtime.env && set +a && rm -f .dev-local/plugins/control.sock && exec target/debug/webdock-plugin-control --config \"$PWD/.dev-local/plugins/plugin-config.json\"'",
    deps=['target/debug/webdock-plugin-control', '.dev-local/runtime.env'],
    resource_deps=['seed', 'core-build'],
    readiness_probe=probe(exec=exec_action(['test', '-S', '.dev-local/plugins/control.sock']), period_secs=1),
    labels=['plugins'],
)

local_resource(
    'plugin-controller',
    serve_cmd="sh -c 'set -a && . .dev-local/runtime.env && set +a && exec target/debug/webdock-plugin-controller --config \"$PWD/.dev-local/plugins/plugin-config.json\"'",
    deps=['target/debug/webdock-plugin-controller', '.dev-local/runtime.env'],
    resource_deps=['seed', 'core-build'],
    labels=['plugins'],
)

# Plugin workloads: Core's agent listener and one enrolled agent for the local k3s.
local_resource(
    'agent-control',
    serve_cmd="sh -c 'set -a && . .dev-local/runtime.env && set +a && export WEBDOCK_WORKLOADS_ENABLED=true && rm -f .dev-local/agent/run/control.sock && exec target/debug/webdock-agent-control --config \"$PWD/.dev-local/agent/agent-control.json\"'",
    deps=['target/debug/webdock-agent-control', '.dev-local/runtime.env'],
    resource_deps=['seed', 'core-build'],
    readiness_probe=probe(exec=exec_action(['test', '-S', '.dev-local/agent/run/control.sock']), period_secs=1),
    labels=['plugins'],
)
local_resource(
    'agent-build',
    cmd='cargo build -p webdock-agent --bin webdock-agent',
    deps=['crates/agent/src', 'crates/agent/Cargo.toml', 'crates/agent-protocol', 'proto/webdock/agent'],
    labels=['plugins'],
)
local_resource(
    'agent',
    serve_cmd="sh -c 'for i in $(seq 30); do target/debug/webdock-agent --config \"$PWD/.dev-local/agent/agent.json\" run && break; sleep 2; done'",
    deps=['target/debug/webdock-agent', '.dev-local/agent/agent.json'],
    resource_deps=['agent-control', 'agent-build', 'kubeconfig'],
    labels=['plugins'],
)

for name, plugin in plugins.items():
    local_resource(
        name + '-build',
        cmd='cargo build --manifest-path plugins/%s/Cargo.toml' % name,
        deps=['plugins/%s/src' % name, 'plugins/%s/Cargo.toml' % name, 'crates/plugin-runtime', 'crates/plugin-protocol'],
        labels=['plugins'],
    )
    local_resource(
        'plugin-' + name,
        # The seed restarts plugin-control too; a plugin that starts before Core's socket is back
        # exits, so retry for a while instead of failing on the restart race.
        serve_cmd="sh -c 'for i in $(seq 30); do rm -f .dev-local/plugins/%s.sock; plugins/%s/target/debug/%s \"$PWD/.dev-local/plugins/%s.json\" && break; sleep 1; done'" % (name, name, plugin['binary'], name),
        deps=['plugins/%s/target/debug/%s' % (name, plugin['binary']), '.dev-local/plugins/%s.json' % name],
        resource_deps=['plugin-control', name + '-build'] + plugin['deps'],
        readiness_probe=probe(exec=exec_action(['test', '-S', '.dev-local/plugins/%s.sock' % name]), period_secs=1),
        labels=['plugins'],
    )

local_resource(
    'core',
    serve_cmd="sh -c 'set -a && . .dev-local/runtime.env && set +a && exec target/debug/webdock-control-plane'",
    # Restarts after every successful rebuild of the binary.
    deps=['target/debug/webdock-control-plane', '.dev-local/runtime.env'],
    resource_deps=['seed'],
    readiness_probe=probe(http_get=http_get_action(port=3130, host='127.0.0.1', path='/readyz'), period_secs=2),
    links=[link('http://127.0.0.1:3130/healthz', 'healthz')],
    labels=['core'],
)

local_resource(
    'console',
    serve_cmd='bun --bun vite --host 127.0.0.1 --port 3121 --strictPort',
    serve_dir='apps/console',
    serve_env=tls_env,
    # The plugin UI glob is resolved at start; a new plugin restarts the dev server once.
    deps=['plugins/%s/manifest.json' % name for name in plugins],
    resource_deps=['core'],
    links=[link(scheme + '://127.0.0.1:3121', 'Console')],
    labels=['ui'],
)

# Password, MFA and passkey screens run on the canonical auth origin; WebAuthn needs `localhost`.
local_resource(
    'account',
    serve_cmd='bun --bun vite --host localhost --port 3125 --strictPort',
    serve_dir='apps/console',
    serve_env=tls_env,
    # The plugin UI glob is resolved at start; a new plugin restarts the dev server once.
    deps=['plugins/%s/manifest.json' % name for name in plugins],
    resource_deps=['core'],
    links=[link(scheme + '://localhost:3125/account', 'Account')],
    labels=['ui'],
)
