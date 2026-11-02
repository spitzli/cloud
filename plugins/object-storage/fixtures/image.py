#!/usr/bin/env python3
"""Check the actual root build context and a previously built local image; never publish."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]


def run(*args, expected=0):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != expected:
        raise SystemExit("Object Storage image smoke failed; no private diagnostics printed")
    return result.stdout.strip()


def main():
    if len(sys.argv) != 2 or not sys.argv[1].startswith("sha256:"):
        raise SystemExit("Pass the exact locally built image sha256 ID")
    image = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix="wd-object-context-") as destination:
        run("docker", "buildx", "build", "--target", "source", "--output", "type=local,dest=" + destination,
            "-f", "plugins/object-storage/Dockerfile", ".")
        context = Path(destination)
        allowed = {"Cargo.toml", "plugins/object-storage/Cargo.toml", "plugins/object-storage/Cargo.lock",
                   "plugins/object-storage/manifest.json", "proto/webdock/plugin/v1/plugin.proto",
                   "crates/plugin-runtime/build.rs"}
        crates = ["plugin-kit", "plugin-protocol", "plugin-runtime"]
        allowed.update("crates/" + name + "/Cargo.toml" for name in crates)
        sources = ["crates/" + name + "/src/" for name in crates] + ["plugins/object-storage/src/"]
        files = [p for p in context.rglob("*") if p.is_file() or p.is_symlink()]
        for path in files:
            relative = path.relative_to(context).as_posix()
            source = relative.endswith(".rs") and any(relative.startswith(prefix) for prefix in sources)
            migration = relative.startswith("plugins/object-storage/migrations/") and relative.endswith(".sql")
            if path.is_symlink() or not (relative in allowed or source or migration):
                raise SystemExit("Default-deny Docker build context violation")
        if not (context / "plugins/object-storage/src/main.rs").is_file():
            raise SystemExit("Missing plugin build inputs")
        print("Default-deny source context verified: " + str(len(files)) + " build inputs")
    if run("docker", "image", "inspect", image, "--format", "{{.Config.User}}") != "65532:65532":
        raise SystemExit("Runtime UID/GID mismatch")
    run("docker", "run", "--rm", "--pull=never", "--network=none", "--read-only", "--cap-drop=ALL",
        "--security-opt=no-new-privileges", "--entrypoint=/bin/sh", image, "-ec",
        'test "$(id -u)" = 65532; test "$(id -g)" = 65532; '
        'test -s /etc/ssl/certs/ca-certificates.crt; '
        'openssl verify -CAfile /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ISRG_Root_X1.pem; '
        'if touch /tmp/read-only-probe 2>/dev/null; then exit 1; fi')
    result = subprocess.run(["docker", "run", "--rm", "--pull=never", "--network=none", "--read-only",
                             "--cap-drop=ALL", "--security-opt=no-new-privileges", image],
                            capture_output=True, text=True)
    if result.returncode != 1 or result.stdout or result.stderr.strip() != "object_storage_plugin_start_failed":
        raise SystemExit("Native entrypoint/redacted failure smoke failed")
    print("Local image UID/GID, read-only runtime, TLS roots and native entrypoint smoke passed: " + image)


if __name__ == "__main__":
    main()
