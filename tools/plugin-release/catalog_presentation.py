"""Read-only acceptance of first-party plugin presentation after registry publication."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.request


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_presentation(detail, slug, source_sha, fetch):
    require(re.fullmatch(r"[a-z][a-z0-9-]{0,63}", slug), "Invalid plugin identity")
    require(re.fullmatch(r"[a-f0-9]{40}", source_sha), "Immutable source commit required")
    repository = "https://github.com/spitzli/cloud-plugin-" + slug
    base = repository.replace("github.com", "raw.githubusercontent.com") + "/" + source_sha + "/"
    require(detail.get("id") == slug and detail.get("repoUrl") == repository, "Wrong registry entry")
    require(isinstance(detail.get("readmeHtml"), str)
            and re.sub(r"<[^>]*>", "", detail["readmeHtml"]).strip(), "Published README is missing")
    gallery = detail.get("screenshots")
    require(isinstance(gallery, list) and 1 <= len(gallery) <= 12, "Published gallery is missing")
    urls = [("icon", detail.get("iconUrl")), ("readme", base + "README.md")]
    for screenshot in gallery:
        require(isinstance(screenshot, dict) and isinstance(screenshot.get("alt"), str)
                and screenshot["alt"].strip(), "Screenshot requires descriptive alternative text")
        urls.append(("screenshot", screenshot.get("url")))
    proof = []
    for kind, url in urls:
        require(isinstance(url, str) and url.startswith(base)
                and not any(c in url[len(base):] for c in ["?", "#", "%", "\\"])
                and all(p not in ["", ".", ".."] for p in url[len(base):].split("/")),
                "Asset must belong to the reviewed immutable source")
        status, content_type, data = fetch(url)
        require(status == 200 and data, "Published asset cannot be loaded: " + kind)
        if kind != "readme":
            require(content_type.split(";", 1)[0] in ["image/png", "image/svg+xml", "image/webp", "image/jpeg"],
                    "Published image returned a non-image response")
        proof.append({"kind": kind, "url": url, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    return {"plugin": slug, "sourceSHA": source_sha, "checkedAssets": len(proof),
            "renderedReadmePresent": True, "assets": proof}


def fetch(url):
    with urllib.request.urlopen(url, timeout=25) as response:
        data = response.read(8 * 1024 * 1024 + 1)
        require(len(data) <= 8 * 1024 * 1024, "Published asset exceeds acceptance limit")
        return response.status, response.headers.get("Content-Type", ""), data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plugin", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    require(re.fullmatch(r"[a-z][a-z0-9-]{0,63}", args.plugin), "Invalid plugin identity")
    _, _, raw = fetch("https://registry.spitzli.cloud/api/plugins/" + args.plugin)
    proof = validate_presentation(json.loads(raw), args.plugin, args.source_sha, fetch)
    result = json.dumps(proof, indent=2) + "\n"
    if args.output:
        args.output.write_text(result)
    print(result, end="")


if __name__ == "__main__":
    main()
