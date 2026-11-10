#!/usr/bin/env python3
"""Rename platform environment keys without evaluating or revealing their values."""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import sys

LIMIT = 1024 * 1024
LEGACY_REFERENCE = re.compile(r"\$(?:\{)?(?:NEXT_PUBLIC_)?WEBDOCK_[A-Za-z0-9_]+")
ASSIGNMENT = re.compile(r"^(\s*(?:export\s+)?)([A-Za-z_][A-Za-z0-9_]*)(\s*=)")


class InvalidEnvironment(ValueError):
    pass


def canonical(name):
    for prefix in ("NEXT_PUBLIC_WEBDOCK_", "WEBDOCK_"):
        if name.startswith(prefix) and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            return name.replace("WEBDOCK_", "SPITZLI_", 1)
    return name


def register(name, seen, changes):
    target = canonical(name)
    if target in seen:
        raise InvalidEnvironment("Duplicate or conflicting environment key: " + target)
    seen.add(target)
    if target != name:
        changes.append({"source": name, "canonical": target})
    return target


def reject_reference(value):
    if LEGACY_REFERENCE.search(value):
        raise InvalidEnvironment("Resolve renamed-variable interpolation before migration")


def quoted_value(value, quote=None):
    if quote is None:
        value = value.lstrip(" \t")
        if not value or value[0] not in "\"'`":
            if value.rstrip("\r\n").endswith("\\"):
                raise InvalidEnvironment("Unquoted line continuation is unsupported")
            reject_reference(value.split("#", 1)[0])
            return None
        quote, value = value[0], value[1:]
    escaped = False
    for index, char in enumerate(value):
        if char == quote and not escaped:
            rest = value[index + 1:].strip()
            if rest and not rest.startswith("#"):
                raise InvalidEnvironment("Unexpected content after quoted value")
            reject_reference(value[:index])
            return None
        escaped = char == "\\" and not escaped
    reject_reference(value)
    return quote


def dotenv(text):
    seen, changes, output, quote = set(), [], [], None
    for number, line in enumerate(text.splitlines(keepends=True), 1):
        if quote:
            quote = quoted_value(line, quote)
            output.append(line)
            continue
        bom = "\ufeff" if number == 1 and line.startswith("\ufeff") else ""
        source = line[len(bom):]
        if not source.strip() or source.lstrip().startswith("#"):
            output.append(line)
            continue
        match = ASSIGNMENT.match(source)
        if not match:
            raise InvalidEnvironment(f"Invalid environment assignment on line {number}")
        name = register(match[2], seen, changes)
        value = source[match.end():]
        quote = quoted_value(value)
        output.append(bom + source[:match.start(2)] + name + source[match.end(2):])
    if quote:
        raise InvalidEnvironment("Unterminated quoted environment value")
    return "".join(output), changes, len(seen)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise InvalidEnvironment("Duplicate JSON key")
        result[key] = value
    return result


def json_environment(text):
    value = json.loads(text, object_pairs_hook=unique_object)
    if not isinstance(value, dict):
        raise InvalidEnvironment("JSON environment must be an object")
    changes, count = [], 0

    def convert(values):
        nonlocal count
        if not isinstance(values, dict) or not all(isinstance(v, str) for v in values.values()):
            raise InvalidEnvironment("Environment values must remain strings")
        seen, result = set(), {}
        for key, data in values.items():
            result[register(key, seen, changes)] = data
        count += len(seen)
        return result

    if value.get("kind") == "Secret":
        if value.get("apiVersion") != "v1":
            raise InvalidEnvironment("Expected a v1 Kubernetes Secret")
        for field in ("data", "stringData"):
            if field in value:
                value[field] = convert(value[field])
        data_names = set(value.get("data", {}))
        if data_names.intersection(value.get("stringData", {})):
            raise InvalidEnvironment("Conflicting data and stringData keys")
    else:
        value = convert(value)
    return json.dumps(value, ensure_ascii=False, indent=2) + "\n", changes, count


def read_private(path):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as handle:
        metadata = os.fstat(handle.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077:
            raise InvalidEnvironment("Owned private source file required")
        data = handle.read(LIMIT + 1)
    if len(data) > LIMIT or b"\0" in data:
        raise InvalidEnvironment("Environment file exceeds supported bounds")
    return data.decode("utf-8")


def write_private(path, text):
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        metadata = os.fstat(parent)
        if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
            raise InvalidEnvironment("Owned output directory without group/world write required")
        descriptor = os.open(path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent)
        try:
            with os.fdopen(descriptor, "wb") as output:
                os.fchmod(output.fileno(), 0o600)
                output.write(text.encode("utf-8"))
                output.flush()
                os.fsync(output.fileno())
        except BaseException:
            os.unlink(path.name, dir_fd=parent)
            raise
    finally:
        os.close(parent)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--format", choices=["dotenv", "json"], default="dotenv")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if not args.dry_run and not args.output:
        parser.error("--output is required unless --dry-run is used")
    try:
        text = read_private(args.input)
        result, changes, count = (dotenv if args.format == "dotenv" else json_environment)(text)
        if not args.dry_run:
            write_private(args.output, result)
    except (OSError, ValueError):
        print("Environment migration refused: review source syntax, conflicting keys and private-file permissions; no existing file was replaced.", file=sys.stderr)
        return 1
    print(json.dumps({"variables": count, "renamed": changes, "dryRun": args.dry_run, "output": str(args.output) if args.output else None}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
