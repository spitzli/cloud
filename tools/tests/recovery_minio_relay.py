#!/usr/bin/env python3
"""Bounded raw-byte relay for the private, disposable recovery fixture."""
import argparse
import asyncio
import contextlib
import json
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import sys


class RelayError(Exception):
    pass


def private_parent(path):
    path = Path(path)
    if not path.is_absolute() or path.resolve() != path:
        raise RelayError("socket-parent-path")
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        raise RelayError("socket-parent-metadata")


def socket_identity(path, owner):
    metadata = Path(path).lstat()
    if not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != owner or stat.S_IMODE(metadata.st_mode) != 0o600 or metadata.st_nlink != 1:
        raise RelayError("socket-metadata")
    return metadata.st_dev, metadata.st_ino


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--mode", choices=("host", "inner"), required=True)
    result.add_argument("--socket", type=Path, required=True)
    result.add_argument("--port", type=int, required=True, help="Host MinIO port or inner loopback listening port")
    result.add_argument("--socket-owner-uid", type=int, default=os.getuid())
    result.add_argument("--peer-uid", type=int, default=0)
    result.add_argument("--max-connections", type=int, default=4)
    result.add_argument("--max-accepted", type=int, default=128)
    result.add_argument("--max-bytes", type=int, default=512 * 1024 * 1024)
    result.add_argument("--deadline", type=float, default=300)
    result.add_argument("--idle", type=float, default=10)
    result.add_argument("--connect-timeout", type=float, default=2)
    return result


def validate(args):
    if not args.socket.is_absolute() or ".." in args.socket.parts:
        raise RelayError("socket-path")
    if len(os.fsencode(args.socket)) > 107:
        raise RelayError("socket-path-length")
    if not 1024 <= args.port <= 65535 or not 0 <= args.socket_owner_uid <= 4294967295 or not 0 <= args.peer_uid <= 4294967295:
        raise RelayError("endpoint-bound")
    if not 1 <= args.max_connections <= 4 or not 1 <= args.max_accepted <= 128 or not 1 <= args.max_bytes <= 512 * 1024 * 1024:
        raise RelayError("resource-bound")
    if not 0 < args.deadline <= 300 or not 0 < args.idle <= 10 or not 0 < args.connect_timeout <= 2:
        raise RelayError("time-bound")


class Relay:
    def __init__(self, args):
        validate(args)
        self.args = args
        self.stop = asyncio.Event()
        self.tasks = set()
        self.accepted = self.active = self.bytes = self.idle_closed = 0
        self.failure = None
        self.identity = None

    def fail(self, category):
        self.failure = self.failure or category
        self.stop.set()

    async def transfer(self, reader, writer):
        while True:
            block = await asyncio.wait_for(reader.read(65536), self.args.idle)
            if not block:
                with contextlib.suppress(OSError, RuntimeError, NotImplementedError):
                    writer.write_eof()
                return
            if self.bytes + len(block) > self.args.max_bytes:
                raise RelayError("byte-bound")
            self.bytes += len(block)
            writer.write(block)
            await asyncio.wait_for(writer.drain(), self.args.idle)

    async def handle(self, reader, writer):
        task = asyncio.current_task()
        self.tasks.add(task)
        upstream = None
        pumps = []
        admitted = False
        try:
            if self.active >= self.args.max_connections or self.accepted >= self.args.max_accepted:
                raise RelayError("connection-bound")
            if self.args.mode == "host":
                peer = writer.get_extra_info("socket")
                _, uid, _ = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
                if uid != self.args.peer_uid:
                    raise RelayError("peer-uid")
                connection = asyncio.open_connection("127.0.0.1", self.args.port)
            else:
                if socket_identity(self.args.socket, self.args.socket_owner_uid) != self.identity:
                    raise RelayError("socket-replaced")
                connection = asyncio.open_unix_connection(str(self.args.socket))
            self.active += 1
            self.accepted += 1
            admitted = True
            try:
                remote_reader, upstream = await asyncio.wait_for(connection, self.args.connect_timeout)
            except asyncio.TimeoutError:
                raise RelayError("connect-timeout") from None
            pumps = [asyncio.create_task(self.transfer(reader, upstream)), asyncio.create_task(self.transfer(remote_reader, writer))]
            await asyncio.gather(*pumps)
        except asyncio.TimeoutError:
            # HTTP keep-alive connections may become idle; close them without retrying bytes.
            self.idle_closed += 1
        except RelayError as error:
            self.fail(str(error))
        except (OSError, RuntimeError, ValueError):
            self.fail("connection-io")
        finally:
            for pump in pumps:
                if not pump.done():
                    pump.cancel()
            await asyncio.gather(*pumps, return_exceptions=True)
            for stream in (writer, upstream):
                if stream is not None:
                    stream.close()
                    with contextlib.suppress(OSError, RuntimeError):
                        await asyncio.wait_for(stream.wait_closed(), 1)
            if admitted:
                self.active -= 1
            self.tasks.discard(task)

    def receipt(self, stage):
        print(json.dumps({"stage": stage, "mode": self.args.mode, "accepted": self.accepted, "bytes": self.bytes, "idleClosed": self.idle_closed, "failure": self.failure, "socketIdentity": list(self.identity) if self.identity else None}), flush=True)

    async def run(self):
        server = None
        loop = asyncio.get_running_loop()
        try:
            if self.args.mode == "host":
                private_parent(self.args.socket.parent)
                if self.args.socket.exists() or self.args.socket.is_symlink():
                    raise RelayError("socket-exists")
                # Python 3.13+ otherwise unlinks before our ownership-checked cleanup.
                cleanup = {"cleanup_socket": False} if sys.version_info >= (3, 13) else {}
                server = await asyncio.start_unix_server(self.handle, path=str(self.args.socket), start_serving=False, **cleanup)
                self.args.socket.chmod(0o600)
                self.identity = socket_identity(self.args.socket, os.getuid())
            else:
                self.identity = socket_identity(self.args.socket, self.args.socket_owner_uid)
                server = await asyncio.start_server(self.handle, host="127.0.0.1", port=self.args.port, start_serving=False)
            for number in (signal.SIGINT, signal.SIGTERM):
                loop.add_signal_handler(number, self.stop.set)
            await server.start_serving()
            self.receipt("ready")
            try:
                await asyncio.wait_for(self.stop.wait(), self.args.deadline)
            except asyncio.TimeoutError:
                self.fail("deadline")
        finally:
            if server is not None:
                server.close()
                await server.wait_closed()
            for task in list(self.tasks):
                task.cancel()
            await asyncio.gather(*list(self.tasks), return_exceptions=True)
            if self.args.mode == "host" and self.identity is not None:
                if socket_identity(self.args.socket, os.getuid()) != self.identity:
                    self.fail("socket-replaced")
                else:
                    self.args.socket.unlink()
            self.receipt("closed")
        return 1 if self.failure else 0


def main():
    os.umask(0o077)
    try:
        return asyncio.run(Relay(parser().parse_args()).run())
    except (RelayError, OSError):
        print(json.dumps({"stage": "failed", "category": "relay-setup-or-custody"}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
