#!/usr/bin/env python3
"""Owned, bounded TLS hop to the existing loopback MinIO. Never starts an S3 server."""
import http.client
import http.server
import ipaddress
import json
import os
import ssl
import stat
import sys
import time


def validate(config):
    assert set(config) == {"listen", "port", "domain", "certificate", "key", "deadline", "upstreamIP"}
    address = ipaddress.ip_address(config["listen"])
    assert address.version == 4 and (address.is_unspecified or address.is_private and not address.is_loopback)
    upstream = ipaddress.ip_address(config["upstreamIP"])
    assert upstream.version == 4 and upstream.is_private and not upstream.is_loopback and not upstream.is_unspecified
    run, suffix = config["domain"].split(".", 1)
    assert len(run) == 12 and all(c in "0123456789abcdef" for c in run)
    assert suffix == "native-archives.test"
    assert 1024 <= config["port"] <= 65535
    assert time.time() < config["deadline"] <= time.time() + 4 * 3600
    for name in ("certificate", "key"):
        assert os.path.isabs(config[name])
        info = os.lstat(config[name])
        assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.geteuid()
        assert info.st_mode & 0o077 == 0
    return config


def run(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        info = os.fstat(source.fileno())
        assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0
        config = validate(json.load(source))

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def forward(self):
            upstream = None
            try:
                assert time.time() < config["deadline"]
                assert self.headers.get("Host") == config["domain"] + ":" + str(config["port"])
                assert self.path.startswith("/") and not self.path.startswith("//")
                assert not self.headers.get("Transfer-Encoding")
                size = int(self.headers.get("Content-Length", "0"))
                assert 0 <= size <= 16 * 1024 * 1024
                self.connection.settimeout(15)
                upstream = http.client.HTTPConnection(config["upstreamIP"], 9000, timeout=15)
                headers = {k: v for k, v in self.headers.items() if k.lower() not in ("connection", "proxy-connection", "expect")}
                upstream.putrequest(self.command, self.path, skip_host=True, skip_accept_encoding=True)
                for key, value in headers.items():
                    upstream.putheader(key, value)
                upstream.endheaders()
                remaining = size
                while remaining:
                    data = self.rfile.read(min(65536, remaining))
                    assert data
                    upstream.send(data)
                    remaining -= len(data)
                response = upstream.getresponse()
                self.send_response(response.status)
                for key, value in response.getheaders():
                    if key.lower() not in ("connection", "transfer-encoding", "server", "date"):
                        self.send_header(key, value)
                self.send_header("Connection", "close")
                self.end_headers()
                if self.command != "HEAD":
                    transferred = 0
                    while data := response.read(65536):
                        transferred += len(data)
                        assert transferred <= 16 * 1024 * 1024
                        self.wfile.write(data)
                self.close_connection = True
            except Exception:
                self.close_connection = True
            finally:
                if upstream:
                    upstream.close()

        do_GET = do_HEAD = do_PUT = do_POST = do_DELETE = forward

    server = http.server.ThreadingHTTPServer((config["listen"], config["port"]), Handler)
    server.timeout = 1
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(config["certificate"], config["key"])
    server.socket = context.wrap_socket(server.socket, server_side=True)
    while time.time() < config["deadline"]:
        server.handle_request()
    server.server_close()


if __name__ == "__main__":
    try:
        run(sys.argv[1])
    except Exception:
        raise SystemExit("native_object_storage_proxy_failed") from None
