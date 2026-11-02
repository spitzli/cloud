#!/usr/bin/env python3
"""Disposable CMS acceptance. Only resources bearing this run's random label are removed."""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
LABEL = "dev.webdock.cms-acceptance"
PREFIX = "webdock-cms-acceptance-"
BUN_IMAGE = "oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288"


def clean_environment(environment):
    """Never inherit application, database, dotenv or Node preload configuration."""
    allowed = ("PATH", "HOME", "DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG",
               "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH", "XDG_RUNTIME_DIR", "SSH_AUTH_SOCK", "LANG", "LC_ALL")
    return {key: environment[key] for key in allowed if key in environment}


def owned(resource, run_id):
    return (resource.get("Labels") or resource.get("Config", {}).get("Labels") or {}).get(LABEL) == run_id


def fixture_path(directory, run_id):
    resolved = Path(directory).resolve()
    if not re.fullmatch(r"[a-f0-9]{16}", run_id) or resolved.name != PREFIX + run_id:
        raise RuntimeError("Refusing cleanup outside the exact owned fixture directory")
    return resolved


class Fixture:
    def __init__(self, directory, run_id, resources=None):
        self.directory = fixture_path(directory, run_id)
        self.run_id, self.resources = run_id, resources or []
        self.environment = clean_environment(os.environ)
        self.counter = 0
        self.redactions = []
        self.persist()

    def private(self, name, value):
        target = self.directory / name
        descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w") as handle:
            handle.write(value)
        os.chmod(target, 0o600)
        return target

    def persist(self):
        self.private("fixture.json", json.dumps({"version": 1, "run_id": self.run_id,
                     "directory": str(self.directory), "resources": self.resources}, indent=2))

    def run(self, step, arguments, input=None, timeout=240, check=True):
        self.counter += 1
        result = subprocess.run(arguments, input=input, text=True, capture_output=True,
                                cwd=ROOT, env=self.environment, timeout=timeout)
        output = result.stdout + result.stderr
        for value in self.redactions:
            output = output.replace(value, "[redacted]")
        output = re.sub(r"postgres(?:ql)?://[^\s\"']+", "[database URL redacted]", output)
        self.private(f"{self.counter:03d}-{re.sub('[^a-z0-9-]', '-', step.lower())}.log", output)
        if check and result.returncode:
            raise RuntimeError(f"{step} failed (exit {result.returncode}); private logs: {self.directory}")
        return result

    def inspect(self, kind, identifier):
        result = self.run("inspect", ["docker", kind, "inspect", identifier], check=False)
        return json.loads(result.stdout)[0] if result.returncode == 0 else None

    def create(self, kind, name, arguments):
        if self.inspect(kind, name) is not None:
            raise RuntimeError(f"Refusing to reuse existing {kind}: {name}")
        resource = {"kind": kind, "name": name, "id": None}
        self.resources.append(resource)
        self.persist()
        result = self.run(f"create-{kind}", ["docker", kind, "create", "--label", f"{LABEL}={self.run_id}", *arguments, name] if kind != "container" else ["docker", "create", "--name", name, "--label", f"{LABEL}={self.run_id}", *arguments])
        resource["id"] = result.stdout.strip()
        self.persist()
        return resource

    def remove(self, resource):
        identifier = resource.get("id") or resource["name"]
        current = self.inspect(resource["kind"], identifier)
        if current is None:
            return
        if not owned(current, self.run_id):
            raise RuntimeError(f"Ownership label mismatch; refusing to remove {resource['kind']}")
        command = ["docker", resource["kind"], "rm"]
        if resource["kind"] == "container":
            command += ["--force", "--volumes"]
        self.run("remove-owned-resource", command + [identifier])

    def cleanup(self, remove_directory=False):
        for resource in reversed(self.resources):
            self.remove(resource)
        for resource in self.resources:
            if self.inspect(resource["kind"], resource.get("id") or resource["name"]) is not None:
                raise RuntimeError("Owned fixture resource remained after cleanup")
        print(f"Owned CMS fixture {self.run_id}: {len(self.resources)} exact resources confirmed absent after cleanup.", flush=True)
        if remove_directory:
            shutil.rmtree(fixture_path(self.directory, self.run_id))

    def job(self, name, image, arguments, envfile=None, mounts=(), user=None, network=None, memory="1024m"):
        options = ["--read-only", "--tmpfs", "/tmp:rw,noexec,nosuid,size=128m", "--memory", memory,
                   "--memory-swap", memory, "--cpus", "2", "--pids-limit", "256", "--workdir", "/app"]
        if user is not None:
            options += ["--user", user]
        if envfile:
            options += ["--env-file", str(envfile)]
        if network:
            options += ["--network", network]
        for mount in mounts:
            options += ["--volume", mount]
        container = self.create("container", f"{PREFIX}{self.run_id}-{name}", [*options, image, *arguments])
        self.run(name, ["docker", "start", "--attach", container["id"]])
        code = self.inspect("container", container["id"])["State"]["ExitCode"]
        if code:
            raise RuntimeError(f"{name} exited {code}; private logs: {self.directory}")
        self.remove(container)


def main(s3_config=None, mc=None, access=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3", action="store_true", help="Fresh private S3 media using existing MinIO; source process recreation")
    parser.add_argument("--image", help="Already built local CMS image; never rebuilt or deployed")
    parser.add_argument("--keep", action="store_true", help="Retain this fixture; use --image for a browser endpoint")
    parser.add_argument("--cleanup", type=Path, help="Remove only resources owned by a retained fixture")
    parser.add_argument("--postgres-image", default="postgres:18-alpine")
    parser.add_argument("--bun-image", default=BUN_IMAGE, help="Bun runtime image for migrations and tests")
    args = parser.parse_args()
    if args.s3 and args.cleanup:
        raise RuntimeError("--s3 and --cleanup are separate operations")
    if args.s3 and args.keep:
        raise RuntimeError("S3 fixture has bounded lifetime; --keep is not supported")
    if args.s3 and s3_config is None:
        import importlib.util
        spec = importlib.util.spec_from_file_location("owned_s3", ROOT / "tools/tests/object-storage.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.main(callback=lambda config, admin, key: main(config, admin, key), registry=True)
    if args.cleanup:
        manifest = args.cleanup / "fixture.json"
        if manifest.stat().st_uid != os.getuid() or manifest.stat().st_mode & 0o077:
            raise RuntimeError("Fixture manifest must be owned by this user and private")
        state = json.loads(manifest.read_text())
        directory = fixture_path(args.cleanup, state["run_id"])
        if state["directory"] != str(directory) or state["version"] != 1:
            raise RuntimeError("Invalid fixture manifest")
        Fixture(directory, state["run_id"], state["resources"]).cleanup(True)
        print("Owned fixture removed.")
        return
    run_id = secrets.token_hex(8)
    directory = Path(tempfile.mkdtemp(prefix=PREFIX))
    exact = directory.with_name(PREFIX + run_id)
    directory.rename(exact)
    os.chmod(exact, 0o700)
    fixture = Fixture(exact, run_id)
    completed = False
    try:
        bun_image = args.bun_image
        resolved_images = {}
        for image in (args.postgres_image, bun_image):
            inspected = fixture.run("inspect-image", ["docker", "image", "inspect", image], check=False)
            if inspected.returncode:
                print(f"Pulling test image {image}…", flush=True)
                fixture.run("pull-image", ["docker", "pull", image], timeout=600)
                inspected = fixture.run("inspect-image", ["docker", "image", "inspect", image])
            resolved_images[image] = json.loads(inspected.stdout)[0]["Id"]
        bun_image, postgres_image = resolved_images[bun_image], resolved_images[args.postgres_image]
        image_id = None
        if args.image:
            candidate = json.loads(fixture.run("inspect-candidate", ["docker", "image", "inspect", args.image]).stdout)[0]
            if candidate["Config"]["User"] != "65532:65532":
                raise RuntimeError("CMS candidate image must configure UID/GID 65532:65532")
            image_id = candidate["Id"]
        network = fixture.create("network", PREFIX + run_id, [] if args.keep or args.s3 else ["--internal"])["id"]
        volume = fixture.create("volume", PREFIX + run_id, [])["id"]
        # Check both container runtimes independently of the host PATH version.
        fixture.job("bun-version", bun_image, ["bun", "-e", "if(!Bun.version.startsWith('1.4.'))process.exit(1)"], memory="256m")
        pg_version = fixture.run("postgres-version", ["docker", "run", "--rm", "--label", f"{LABEL}={run_id}", "--entrypoint", "postgres", postgres_image, "--version"]).stdout
        if not re.search(r"PostgreSQL\) 18\.", pg_version):
            raise RuntimeError("PostgreSQL 18 is required")
        pg_uid = fixture.run("postgres-uid", ["docker", "run", "--rm", "--label", f"{LABEL}={run_id}", "--entrypoint", "id", postgres_image, "-u", "postgres"]).stdout.strip()
        if not re.fullmatch(r"[0-9]{1,10}", pg_uid):
            raise RuntimeError("Invalid PostgreSQL image user ID")
        certs = exact / "tls"
        certs.mkdir(mode=0o755)
        fixture.run("certificate", ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(certs / "server.key"), "-out", str(certs / "server.crt"), "-days", "2", "-subj", "/CN=postgres", "-addext", "subjectAltName=DNS:postgres,DNS:localhost,IP:127.0.0.1"])
        os.chmod(certs / "server.key", 0o600)
        fixture.job("certificate-owner", bun_image, ["sh", "-c", f"chown {pg_uid}:{pg_uid} /tls/server.key && chmod 600 /tls/server.key"], mounts=[f"{certs}:/tls"], user="0", memory="256m")
        passwords = [secrets.token_hex(32) for _ in range(3)]
        fixture.redactions += passwords
        pg_env = fixture.private("postgres.env", f"POSTGRES_PASSWORD={passwords[0]}\nPOSTGRES_INITDB_ARGS=--auth-host=scram-sha-256\n")
        postgres = fixture.create("container", f"{PREFIX}{run_id}-postgres", ["--network", network, "--network-alias", "postgres", *(["--publish", "127.0.0.1::5432"] if args.s3 else []), "--env-file", str(pg_env), "--memory", "512m", "--memory-swap", "512m", "--cpus", "1", "--pids-limit", "256", "--volume", f"{certs}:/tls:ro", postgres_image, "-c", "ssl=on", "-c", "ssl_cert_file=/tls/server.crt", "-c", "ssl_key_file=/tls/server.key", "-c", "shared_buffers=32MB", "-c", "max_connections=30"])
        fixture.run("start-postgres", ["docker", "start", postgres["id"]])
        for attempt in range(120):
            if not fixture.run("postgres-ready", ["docker", "exec", postgres["id"], "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], check=False).returncode:
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("Disposable PostgreSQL did not become ready")
        def sql(database, statement):
            fixture.run("fixture-sql", ["docker", "exec", "--interactive", postgres["id"], "psql", "--no-psqlrc", "--quiet", "--set", "ON_ERROR_STOP=1", "-U", "postgres", "-d", database], input=statement)
        sql("postgres", f"CREATE ROLE cms_migrate LOGIN PASSWORD '{passwords[1]}'; CREATE ROLE cms_runtime LOGIN PASSWORD '{passwords[2]}'; CREATE DATABASE cms OWNER cms_migrate;")
        sql("cms", "CREATE SCHEMA app AUTHORIZATION cms_migrate; REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT USAGE ON SCHEMA app TO cms_runtime; ALTER DEFAULT PRIVILEGES FOR ROLE cms_migrate IN SCHEMA app GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO cms_runtime; ALTER DEFAULT PRIVILEGES FOR ROLE cms_migrate IN SCHEMA app GRANT USAGE,SELECT ON SEQUENCES TO cms_runtime;")
        common = {"NODE_ENV": "production", "CMS_INSTANCE_ID": f"fixture-{run_id}", "CMS_PROJECT_ID": f"project-{run_id}", "CMS_ORIGIN": "https://cms-fixture.invalid", "CMS_SITE_NAME": "Disposable CMS acceptance", "PAYLOAD_SECRET": secrets.token_hex(32), "CMS_BRIDGE_SECRET": secrets.token_hex(32)}
        fixture.redactions += [common["PAYLOAD_SECRET"], common["CMS_BRIDGE_SECRET"]]
        def envfile(name, values):
            return fixture.private(name, "".join(f"{key}={value}\n" for key, value in values.items()))
        database_host = "postgres:5432"
        job_network = network
        if args.s3:
            port = fixture.inspect("container", postgres["id"])["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostPort"]
            database_host = f"127.0.0.1:{port}"
            job_network = "host"
            storage = json.loads(Path(s3_config).read_text())
            credentials = storage["credentials"]
            fixture.redactions += list(credentials.values())
            common.update({"CMS_MEDIA_STORAGE":"s3", "CMS_ENVIRONMENT_ID":"fixture", "CMS_MEDIA_S3_RESOURCE_ID": "bbba0aa0-2f38-4fb0-a870-81808ccbd1b0", "CMS_MEDIA_S3_ENDPOINT":storage["config"]["endpoint"], "CMS_MEDIA_S3_REGION":storage["config"]["region"], "CMS_MEDIA_S3_BUCKET":storage["config"]["bucket"], "CMS_MEDIA_S3_LOOPBACK_TEST":"true"})
        def dburl(role, password):
            return f"postgresql://{role}:{password}@{database_host}/cms?sslmode=verify-full&sslrootcert=/tls/ca.crt"
        migration_env = envfile("migration.env", {**common, "CMS_MIGRATING": "true", "CMS_MIGRATION_DATABASE_URL": dburl("cms_migrate", passwords[1]), "CMS_BOOTSTRAP_SUBJECT": f"bootstrap-{run_id}", "CMS_BOOTSTRAP_ROLE": "admin", **({"CMS_MEDIA_INITIALIZE_STORAGE":"true"} if args.s3 else {})})
        runtime = {**common, "DATABASE_URL": dburl("cms_runtime", passwords[2]), "CMS_ACCEPTANCE_RUN": "true"}
        if args.s3:
            runtime.update({"CMS_S3_ACCEPTANCE":"true", "CMS_MEDIA_S3_ACCESS_KEY_ID":credentials["accessKeyId"], "CMS_MEDIA_S3_SECRET_ACCESS_KEY":credentials["secretAccessKey"]})
        runtime_env = envfile("runtime.env", runtime)
        mounts = [f"{ROOT}:/app:ro", f"{certs / 'server.crt'}:/tls/ca.crt:ro", f"{volume}:/data"]
        fixture.job("source-volume-owner", bun_image, ["sh", "-c", f"mkdir -p /data/media /data/transfers && chown -R {os.getuid()}:{os.getgid()} /data"], mounts=[f"{volume}:/data"], user="0", memory="256m")
        if not args.s3:
            print("Seeding a preexisting local file and row on the old schema…", flush=True)
            fixture.job("seed-legacy-media", bun_image, ["bun", "apps/cms-instance/scripts/seed-legacy-media.ts"], migration_env, mounts, f"{os.getuid()}:{os.getgid()}", job_network)
        print("Running explicit schema migration with migration-only credentials…", flush=True)
        migration_image = image_id or bun_image
        migration_command = ["bun", "/app/migrate.mjs"] if image_id else ["bun", "apps/cms-instance/src/migrate.ts"]
        fixture.job("migrate", migration_image, migration_command, migration_env, mounts if not image_id else [f"{certs / 'server.crt'}:/tls/ca.crt:ro"], network=job_network)
        fixture.job("migrate-idempotent", migration_image, migration_command, migration_env, mounts if not image_id else [f"{certs / 'server.crt'}:/tls/ca.crt:ro"], network=job_network)
        print("Running existing source acceptance and editor safety tests with DML-only credentials…", flush=True)
        if not args.s3:
            runtime["CMS_LEGACY_ACCEPTANCE"] = "true"
            runtime_env = envfile("runtime.env", runtime)
        mismatch = {**runtime, "CMS_BINDING_REJECTION": "true", "CMS_MEDIA_STORAGE": "s3", "CMS_ENVIRONMENT_ID": "fixture", "CMS_MEDIA_S3_RESOURCE_ID": "bbba0aa0-2f38-4fb0-a870-81808ccbd1b0", "CMS_MEDIA_S3_ENDPOINT": "https://storage.invalid", "CMS_MEDIA_S3_REGION": "test", "CMS_MEDIA_S3_BUCKET": f"owned-mismatch-{run_id}", "CMS_MEDIA_S3_ACCESS_KEY_ID": "unused-fixture", "CMS_MEDIA_S3_SECRET_ACCESS_KEY": "unused-fixture"}
        mismatch_env = envfile("binding-mismatch.env", mismatch)
        fixture.job("binding-rejection", bun_image, ["bun", "test", "./apps/cms-instance/tests/startup-real.test.ts"], mismatch_env, mounts, f"{os.getuid()}:{os.getgid()}", job_network)
        files = "acceptance editor-safety s3-media" if args.s3 else "legacy-media acceptance editor-safety media-trash"
        fixture.job("source-tests", bun_image, ["sh", "-c", f"status=0; for file in {files}; do bun test ./apps/cms-instance/tests/$file.test.ts || status=1; done; exit $status"], runtime_env, mounts, f"{os.getuid()}:{os.getgid()}", job_network)
        if args.s3:
            def s3_phase(phase):
                phase_env = envfile(f"s3-{phase}.env", {**runtime, "CMS_S3_PHASE":phase})
                fixture.job(f"s3-{phase}", bun_image, ["bun", "test", "./apps/cms-instance/tests/s3-media.test.ts"], phase_env, mounts, f"{os.getuid()}:{os.getgid()}", job_network)
            s3_phase("read")
            mc("admin", "user", "disable", "fixture", access)
            try:
                s3_phase("denied")
            finally:
                mc("admin", "user", "enable", "fixture", access)
            s3_phase("read")
            s3_phase("prepare-purge")
            deny_policy = f"wd-cms-deny-delete-{run_id}"
            deny = {"Version": "2012-10-17", "Statement": [{"Effect": "Deny", "Action": ["s3:DeleteObject"], "Resource": [f"arn:aws:s3:::{storage['config']['bucket']}/*"]}]}
            created = attached = False
            try:
                mc("admin", "policy", "create", "fixture", deny_policy, "/dev/stdin", input_text=json.dumps(deny))
                created = True
                mc("admin", "policy", "attach", "fixture", deny_policy, "--user", access)
                attached = True
                s3_phase("denied-purge")
                # Object bytes remain reachable by the fixture administrator while public metadata is trashed.
                remaining = mc("ls", "--recursive", "--json", f"fixture/{storage['config']['bucket']}")
                if not remaining.strip():
                    raise RuntimeError("Denied purge lost all provider objects")
            finally:
                if attached:
                    mc("admin", "policy", "detach", "fixture", deny_policy, "--user", access)
                if created:
                    mc("admin", "policy", "remove", "fixture", deny_policy)
            s3_phase("purge")
            print("Private S3 media process recreation, exact bytes, trash/restore, denied access and hash mismatch passed.", flush=True)
        origin = None
        if image_id:
            fixture.job("image-volume-owner", bun_image, ["sh", "-c", "chown -R 65532:65532 /data"], mounts=[f"{volume}:/data"], user="0", memory="256m")
            image_port = 3000
            image_origin = "http://cms:3000"
            if args.s3:
                import socket
                with socket.socket() as listener:
                    listener.bind(("127.0.0.1", 0))
                    image_port = listener.getsockname()[1]
                image_origin = f"http://127.0.0.1:{image_port}"
            image_env = envfile("image.env", {**runtime, "PORT": str(image_port), "HOSTNAME": "127.0.0.1" if args.s3 else "0.0.0.0", "CMS_IMAGE_TEST_ORIGIN": image_origin})
            def start_image(suffix, environment=image_env):
                container = fixture.create("container", f"{PREFIX}{run_id}-cms-{suffix}", ["--network", job_network, *([] if args.s3 else ["--network-alias", "cms"]), "--read-only", "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m", "--env-file", str(environment), "--memory", "768m", "--memory-swap", "768m", "--cpus", "2", "--pids-limit", "256", *(["--publish", "127.0.0.1::3000"] if args.keep else []), "--volume", f"{certs / 'server.crt'}:/tls/ca.crt:ro", "--volume", f"{volume}:/data", image_id])
                fixture.run("start-image", ["docker", "start", container["id"]])
                return container
            rejected_env = envfile("image-rejected.env", {**mismatch, "PORT": str(image_port), "HOSTNAME": "127.0.0.1" if args.s3 else "0.0.0.0"})
            rejected = start_image("rejected", rejected_env)
            probe_env = envfile("image-rejected-probe.env", {**runtime, "CMS_IMAGE_TEST_ORIGIN": image_origin, "CMS_IMAGE_ACCEPTANCE_PHASE": "binding-rejection"})
            fixture.job("image-binding-rejection", bun_image, ["bun", "test", "./apps/cms-instance/tests/image-acceptance.test.ts"], probe_env, mounts, "65532:65532", job_network)
            fixture.remove(rejected)
            container = start_image("first")
            for phase in ("write", "read"):
                if phase == "read":
                    fixture.remove(container)
                    container = start_image("recreated")
                phase_env = envfile(f"image-{phase}.env", {**runtime, "CMS_IMAGE_TEST_ORIGIN": image_origin, "CMS_IMAGE_ACCEPTANCE_PHASE": phase})
                print(f"Running image HTTP {phase} acceptance…", flush=True)
                fixture.job(f"image-{phase}", bun_image, ["bun", "test", "./apps/cms-instance/tests/image-acceptance.test.ts"], phase_env, mounts, "65532:65532", job_network)
            if args.keep:
                current = fixture.inspect("container", container["id"])
                bindings = current["NetworkSettings"]["Ports"].get("3000/tcp")
                if not bindings:
                    raise RuntimeError("Retained CMS did not receive a loopback port")
                origin = f"http://127.0.0.1:{bindings[0]['HostPort']}"
        completed = True
        print("CMS acceptance passed: fresh PostgreSQL 18/TLS, separate roles, explicit migrations, existing safety tests" + (", exact 10 MiB transport/public hash and container recreation." if image_id else "."), flush=True)
        if args.keep:
            print(f"Retained owned fixture: {exact}" + (f"\nBrowser endpoint: {origin}" if origin else ""))
            print(f"Cleanup: python3 apps/cms-instance/scripts/acceptance.py --cleanup {exact}")
    finally:
        if not args.keep:
            fixture.cleanup(remove_directory=completed)
            if not completed:
                print(f"Owned containers/network/volume removed; private failure logs retained at {exact}", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, OSError) as error:
        print(f"CMS acceptance failed: {error}", file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(f"CMS acceptance failed unexpectedly ({type(error).__name__}); private fixture logs contain step output", file=sys.stderr)
        sys.exit(1)
