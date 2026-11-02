#!/usr/bin/env python3
"""Move workspace directories and their local references after fixture teardown.

Default is a read-only plan. --apply requires the parent-coordinated stop of builds,
servers and fixtures. Reports contain file paths/counts only, never file contents.
"""
from __future__ import annotations
import argparse
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SELF = Path(__file__).resolve().relative_to(ROOT).as_posix()
MOVES = {
    "apps/admin": "legacy/console",
    "apps/api": "legacy/control-plane",
    "apps/web": "apps/website",
    "apps/hosting-agent": "agents/kubernetes",
}
CURRENT_ARCHITECTURE = {
    "cms-module-boundary.md", "clerk-browser-auth.md", "database-module-boundary.md",
    "hosting-provider-boundary.md", "native-mail-boundary.md", "module-installation.md",
    "mail-runtime-boundary.md", "modular-platform-status.md", "selfhost-distribution.md",
    "selfhost-onboarding.md",
}
REMOTE_FIXTURES = {
    "apps/api/tests/ported-auth/setup-git-retry.test.ts",
    "apps/api/tests/ported-auth/git-deployments.test.ts",
    "apps/api/tests/ported-auth/git-github.test.ts",
}
EXTENSIONS = {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".json",
              ".md", ".mdx", ".yaml", ".yml", ".toml", ".sh", ".css", ".scss"}
PATH_PATTERN = re.compile(r"(?<![\w-])apps/(hosting-agent|admin|api|web)(?![\w-])")
URL_PATTERN = re.compile(r"(?:https?|wss?)://[^\s\"'`<>]+")

# These are directory selectors, never role values or public component names.
TARGETED = {
    "scripts/build-icons.mjs": [( '["web", "admin", "auth"]', '["website", "console", "auth"]')],
    "tools/release/verify-architecture.mjs": [('["console", "admin", 3120]', '["console", "console", 3120]')],
    "tools/release/smoke-frontends.py": [("('console', 'admin', 3120)", "('console', 'console', 3120)")],
    "apps/api/tests/frontend-boundary.test.ts": [('["auth", "admin"]', '["auth", "console"]'), ('app === "admin"', 'app === "console"')],
    "apps/admin/tests/git-i18n.test.ts": [('../api/src/modules/hosting', '../control-plane/src/modules/hosting')],
    "apps/admin/tests/setup-i18n.test.ts": [('../api/src/modules/hosting', '../control-plane/src/modules/hosting')],
    "apps/api/scripts/legacy/hosting-local-fixture.ts": [('["admin", adminEnv]', '["console", adminEnv]')],
    "apps/api/scripts/legacy/git-deployments-local-fixture.ts": [('["admin", adminEnv]', '["console", adminEnv]')],
    "apps/api/scripts/legacy/managed-postgresql-local-fixture.ts": [('    admin: {', '    console: {')],
    "apps/cms/scripts/verify-instance-source.mjs": [("|| '../web'", "|| '../website'")],
    "apps/cms/scripts/provision-instance-schemas.mjs": [("webdock:'../web'", "webdock:'../website'")],
    "apps/cms/scripts/import-instance-users.mjs": [("webdock:'../web'", "webdock:'../website'")],
    "apps/hosting-agent/test_service_bindings.py": [("Path(__file__).parents[1].joinpath('api/tests/ported-auth/fixtures/database-ca.crt')", "Path(__file__).parents[2].joinpath('legacy/control-plane/tests/ported-auth/fixtures/database-ca.crt')")],
    "deploy/helm/webdock/templates/platform.yaml": [('{{ if eq $name "console" }}admin{{ else }}auth{{ end }}', '{{ if eq $name "console" }}console{{ else }}auth{{ end }}')],
}


def mapped(name: str) -> str:
    for old, new in MOVES.items():
        if name == old or name.startswith(old + "/"):
            return new + name[len(old):]
    return name


def repository_files() -> list[str]:
    raw = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT)
    return sorted(set(p.decode() for p in raw.split(b"\0") if p))


def excluded(name: str) -> bool:
    path = Path(name)
    parts = path.parts
    if name == SELF or name.endswith(("package-lock.json", "bun.lock", "bun.lockb", ".pot", ".po")):
        return True
    if any(p in {"node_modules", ".next", "dist", ".git", "__snapshots__", "snapshots", "vendor", ".release"} for p in parts):
        return True
    if name.startswith(("docs/verification/", "docs/superpowers/", "docs/operations/", "docs/ux/")):
        return True
    if name.startswith("docs/architecture/") and path.name not in CURRENT_ARCHITECTURE:
        return True
    if path.name in {"baseline.sql", "baseline.sha256"} or "checkpoint" in path.name.lower():
        return True
    if ".env" in path.name or path.name.endswith((".pem", ".key", ".crt", ".dump", ".sql", ".snap")):
        return True
    if path.name.startswith((".api-fixture", ".rls-fixture")) or any(word in path.name.lower() for word in ["credentials", "secrets", "private-values"]):
        return True
    if name.startswith("docs/openapi/"):
        return True
    return not (path.suffix in EXTENSIONS or path.name in {"Dockerfile", ".gitignore", ".dockerignore", ".mintignore"} or path.name.endswith((".Dockerfile", ".dockerignore")))


def rewrite(name: str, source: str) -> str:
    # Remote Git fixtures intentionally describe another repository's apps/api/web.
    if name not in REMOTE_FIXTURES:
        spans = [match.span() for match in URL_PATTERN.finditer(source)]
        def replace(match: re.Match[str]) -> str:
            if any(start <= match.start() < end for start, end in spans):
                return match.group()
            return MOVES[match.group()]
        source = PATH_PATTERN.sub(replace, source)
    for old, new in TARGETED.get(name, []):
        source = source.replace(old, new)
    if name == ".dockerignore" and "!agents/**" not in source.splitlines():
        source = source.replace("!apps/**\n", "!apps/**\n!agents/**\n")
    return source


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="perform the move after all tests/fixtures are stopped")
    parser.add_argument("--report-dir", type=Path, default=Path("/tmp/webdock-workspace-move"))
    args = parser.parse_args()
    if not (ROOT / "package.json").is_file():
        raise SystemExit("Run from the Webdock repository checkout")
    for old, new in MOVES.items():
        if not (ROOT / old).is_dir() or (ROOT / new).exists():
            raise SystemExit(f"Move precondition failed: {old} -> {new}")
    # Fixture metadata moves with directories, but active containers must not be moved out from under tests.
    for marker in (ROOT / "apps/api/.api-fixture.json", ROOT / "apps/api/.rls-fixture.json"):
        if marker.exists():
            raise SystemExit("Tear down active API/RLS fixtures before planning or applying the move")
    changes: dict[str, str] = {}
    inventory = []
    for name in repository_files():
        if excluded(name):
            continue
        path = ROOT / name
        if not path.is_file() or path.is_symlink():
            continue
        try:
            before = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        after = rewrite(name, before)
        if after != before:
            changes[name] = after
        remaining = len(PATH_PATTERN.findall(after))
        if remaining:
            inventory.append({"path": mapped(name), "old_path_occurrences": remaining,
                              "reason": "remote-fixture" if name in REMOTE_FIXTURES else "preserved-url-or-review"})
    if args.apply:
        (ROOT / "agents").mkdir(exist_ok=True)
        for old, new in MOVES.items():
            (ROOT / old).rename(ROOT / new)
        for old_name, value in changes.items():
            (ROOT / mapped(old_name)).write_text(value, encoding="utf-8")
    report = {"applied": args.apply, "moves": MOVES,
              "changed_files": [mapped(name) for name in sorted(changes)],
              "remaining_active_old_paths": inventory,
              "generated_paths_not_rewritten": ["package-lock.json", "packages/i18n/locales"],
              "excluded_records": ["historical architecture/checkpoints", "docs/verification", "docs/operations", "docs/superpowers", "docs/ux", "vendor", "snapshots", "baseline", "OpenAPI"]}
    args.report_dir.mkdir(parents=True, exist_ok=True)
    (args.report_dir / "changed-files.json").write_text(json.dumps({key: report[key] for key in ["applied", "moves", "changed_files"]}, indent=2) + "\n")
    (args.report_dir / "old-path-inventory.json").write_text(json.dumps({key: report[key] for key in ["remaining_active_old_paths", "generated_paths_not_rewritten", "excluded_records"]}, indent=2) + "\n")
    print(json.dumps({"applied": args.apply, "changed_file_count": len(changes),
                      "remaining_active_file_count": len(inventory), "report_directory": str(args.report_dir)}))


if __name__ == "__main__":
    main()
