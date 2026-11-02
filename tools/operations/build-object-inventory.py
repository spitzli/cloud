#!/usr/bin/env python3
"""Read-only correlation of a private Core ledger snapshot and a private mc JSON listing.

This command cannot delete, abort, migrate or upload anything. A candidate report is
not deletion authority. Unknown generations, local-only copies and all references stay held.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import time

ID = re.compile(r"[1-9][0-9]{0,18}\Z")
HASH = re.compile(r"[a-f0-9]{64}\Z")
UUID = r"[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}"
STATES = {"queued", "running", "succeeded", "failed", "cancelled"}


def valid_id(value):
    return isinstance(value, str) and ID.fullmatch(value) is not None


def analyze(ledger, listing, namespace, *, binding, now=None, max_age=300):
    now = time.time() if now is None else now
    if not isinstance(binding, str) or not HASH.fullmatch(binding):
        raise ValueError("invalid target binding")
    if not isinstance(namespace, str) or len(namespace) > 480 or any(part in {".", ".."} or not re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in namespace.split("/")):
        raise ValueError("invalid inventory namespace")
    if not isinstance(ledger, dict) or ledger.get("version") != 1 or type(ledger.get("capturedAt")) is not int or not -5 <= now - ledger["capturedAt"] <= max_age:
        raise ValueError("stale or invalid Core snapshot")
    if not isinstance(ledger.get("builds"), list) or not isinstance(ledger.get("references"), list) or not isinstance(listing, list):
        raise ValueError("incomplete inventory inputs")
    builds = {}
    for build in ledger.get("builds", []):
        if not isinstance(build, dict) or not valid_id(build.get("id")) or not valid_id(build.get("customerID")) or type(build.get("generation")) is not int or build["generation"] < 0 or build.get("status") not in STATES or build["id"] in builds:
            raise ValueError("invalid Core build inventory")
        builds[build["id"]] = build
    references, local_only, errors = {}, set(), []
    for ref in ledger.get("references", []):
        if not isinstance(ref, dict) or not valid_id(ref.get("buildID")) or not valid_id(ref.get("customerID")):
            raise ValueError("invalid Core reference inventory")
        build = builds.get(ref["buildID"])
        if not build or build["customerID"] != ref["customerID"]:
            errors.append("invalid_reference_owner")
            continue
        receipt = ref.get("receipt")
        if receipt is None:
            local_only.add(ref["buildID"])
            continue
        if not isinstance(receipt, dict) or not isinstance(receipt.get("objectKey"), str) or not HASH.fullmatch(str(receipt.get("sha256", ""))) or not HASH.fullmatch(str(receipt.get("binding", ""))) or type(receipt.get("sizeBytes")) is not int or receipt["sizeBytes"] < 1:
            errors.append("invalid_reference")
            continue
        key = receipt["objectKey"]
        parts = key.rsplit("/", 4)
        build = builds.get(ref["buildID"])
        if len(parts) != 5 or parts[1] != ref["customerID"] or parts[2] != ref["buildID"] or not ID.fullmatch(parts[3]) or not re.fullmatch(UUID, parts[4]) or not build or build["customerID"] != ref["customerID"]:
            errors.append("invalid_reference_owner")
            continue
        value = (receipt["binding"], receipt["sizeBytes"])
        if key in references and references[key] != value:
            errors.append("conflicting_reference")
        references[key] = value
    pattern = re.compile(re.escape(namespace) + r"/([1-9][0-9]{0,18})/([1-9][0-9]{0,18})/([1-9][0-9]{0,9})/(" + UUID + r")\Z")
    report = {"version": 1, "capturedAt": ledger["capturedAt"], "generatedAt": int(now), "readOnly": True, "referenced": [], "candidates": [], "held": [], "errors": errors}
    seen = set()
    for obj in listing:
        if not isinstance(obj, dict) or obj.get("status", "success") != "success":
            errors.append("listing_failure")
            continue
        if any(field in obj for field in ("versionId", "versionID", "version_id", "isDeleteMarker")):
            errors.append("versioned_listing")
            continue
        if obj.get("type", "file") != "file":
            continue
        key, size = obj.get("key"), obj.get("size")
        if not isinstance(key, str) or type(size) is not int or size < 0:
            errors.append("invalid_listing")
            continue
        if not key.startswith(namespace + "/"):
            continue
        if key in seen:
            errors.append("duplicate_or_versioned_listing")
            continue
        seen.add(key)
        match = pattern.fullmatch(key)
        if not match:
            report["held"].append({"keyHash": hashlib.sha256(key.encode()).hexdigest(), "size": size, "reason": "unknown_key_shape"})
            continue
        if key in references:
            ref_binding, ref_size = references[key]
            if ref_binding != binding:
                report["held"].append({"key": key, "size": size, "reason": "foreign_binding_reference"})
            else:
                report["referenced"].append({"key": key, "size": size})
                if size != ref_size:
                    errors.append("reference_size_mismatch")
            continue
        customer, build_id, generation, _ = match.groups()
        build, generation = builds.get(build_id), int(generation)
        reason = "unknown_build"
        if build and build["customerID"] == customer:
            if build_id in local_only:
                reason = "local_or_unrecorded_migration_copy"
            elif generation < build["generation"]:
                reason = "superseded_generation"
            elif generation == build["generation"] and build["status"] in {"succeeded", "failed", "cancelled"}:
                reason = "terminal_unreferenced"
            else:
                reason = "active_or_future_generation"
        entry = {"key": key, "size": size, "reason": reason}
        report["candidates" if reason in {"superseded_generation", "terminal_unreferenced"} else "held"].append(entry)
    if errors:
        for entry in report["candidates"]:
            entry["reason"] = "degraded_inventory"
            report["held"].append(entry)
        report["candidates"] = []
    report["status"] = "degraded" if errors else "observed"
    return report


def private_read(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as handle:
        meta = os.fstat(handle.fileno())
        if not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_nlink != 1 or meta.st_mode & 0o077 or meta.st_size > 64 * 1024 * 1024:
            raise ValueError("inventory input must be an owned private bounded regular file")
        data = handle.read(64 * 1024 * 1024 + 1)
        if len(data) > 64 * 1024 * 1024:
            raise ValueError("inventory input grew beyond its bound")
        return data.decode("utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, required=True)
    parser.add_argument("--listing", type=Path, required=True)
    parser.add_argument("--namespace", required=True)
    parser.add_argument("--binding", required=True, help="Exact non-secret retained backend binding fingerprint")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    ledger = json.loads(private_read(args.ledger))
    listing = [json.loads(line) for line in private_read(args.listing).splitlines() if line.strip()]
    report = analyze(ledger, listing, args.namespace, binding=args.binding)
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump(report, handle, indent=2)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    print(json.dumps({"status": report["status"], "referenced": len(report["referenced"]), "candidates": len(report["candidates"]), "held": len(report["held"]), "readOnly": True}))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, json.JSONDecodeError):
        raise SystemExit("inventory refused: inspect private input ownership, schema and snapshot freshness")
