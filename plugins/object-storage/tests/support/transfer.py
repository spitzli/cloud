"""Tiny owned-fixture transport. Signed plans enter on stdin and never appear in diagnostics."""
import json
import sys
import urllib.error
import urllib.request

try:
    data = json.load(sys.stdin)
    body = bytes.fromhex(data["body"]) if data["method"] == "PUT" else None
    request = urllib.request.Request(data["url"], data=body, headers=data["headers"], method=data["method"])
    # Explicitly ignore ambient proxy settings even for the local fixture.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=10) as response:
            print(json.dumps({"status": response.status, "body": response.read(32).hex()}))
    except urllib.error.HTTPError as error:
        print(json.dumps({"status": error.code, "body": ""}))
except Exception:
    sys.exit(1)
