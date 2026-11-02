import asyncio
import importlib.util
import os
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

spec = importlib.util.spec_from_file_location("recovery_minio_relay", Path(__file__).with_name("recovery_minio_relay.py"))
relay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(relay)


def args(*extra):
    return relay.parser().parse_args(["--mode", "host", "--socket", "/tmp/private-relay/socket", "--port", "55900", *extra])


class Writer:
    def __init__(self):
        self.data = bytearray()
        self.eof = False

    def write(self, value):
        self.data.extend(value)

    async def drain(self):
        pass

    def write_eof(self):
        self.eof = True

    def close(self):
        pass

    async def wait_closed(self):
        pass


class RelayTests(unittest.IsolatedAsyncioTestCase):
    async def test_host_lifecycle_removes_only_its_own_socket(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            path = root / "minio.sock"
            instance = relay.Relay(args("--socket", str(path)))
            ready = asyncio.Event()
            instance.receipt = lambda stage: ready.set() if stage == "ready" else None
            loop = asyncio.get_running_loop()
            with patch.object(loop, "add_signal_handler"):
                task = asyncio.create_task(instance.run())
                await asyncio.wait_for(ready.wait(), 1)
                self.assertTrue(path.is_socket())
                instance.stop.set()
                self.assertEqual(await asyncio.wait_for(task, 1), 0)
            self.assertFalse(path.exists())
            self.assertEqual(instance.tasks, set())

    def test_linux_socket_path_bound_is_checked_before_setup(self):
        for path in ("/" + "a" * 107, "/tmp/" + "é" * 52):
            with self.subTest(path_bytes=len(os.fsencode(path))), self.assertRaisesRegex(relay.RelayError, "socket-path-length"):
                relay.Relay(args("--socket", path))
        relay.Relay(args("--socket", "/" + "a" * 106))

    def test_resource_limits_cannot_expand(self):
        for option, value in [("--max-connections", "5"), ("--max-accepted", "129"), ("--max-bytes", "536870913"), ("--deadline", "301"), ("--idle", "11"), ("--connect-timeout", "3"), ("--port", "80")]:
            with self.subTest(option=option), self.assertRaises(relay.RelayError):
                relay.Relay(args(option, value))

    def test_private_socket_identity_rejects_replacement_and_unsafe_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            relay.private_parent(root)
            path = root / "socket"
            with socket.socket(socket.AF_UNIX) as stream:
                stream.bind(str(path))
                path.chmod(0o600)
                original = relay.socket_identity(path, os.getuid())
                path.chmod(0o666)
                with self.assertRaises(relay.RelayError):
                    relay.socket_identity(path, os.getuid())
                path.chmod(0o600)
                path.rename(root / "old")
                path.symlink_to(root / "old")
                with self.assertRaises(relay.RelayError):
                    relay.socket_identity(path, os.getuid())
                self.assertIsInstance(original, tuple)

    async def test_binary_bytes_and_half_close_are_preserved(self):
        reader = asyncio.StreamReader()
        payload = bytes(range(256)) * 1024
        reader.feed_data(payload)
        reader.feed_eof()
        writer = Writer()
        instance = relay.Relay(args())
        await instance.transfer(reader, writer)
        self.assertEqual(writer.data, payload)
        self.assertTrue(writer.eof)
        self.assertEqual(instance.bytes, len(payload))

    async def test_shared_byte_budget_prevents_second_direction_overflow(self):
        instance = relay.Relay(args("--max-bytes", "5"))
        first = asyncio.StreamReader()
        first.feed_data(b"abc")
        first.feed_eof()
        await instance.transfer(first, Writer())
        second = asyncio.StreamReader()
        second.feed_data(b"def")
        writer = Writer()
        with self.assertRaisesRegex(relay.RelayError, "byte-bound"):
            await instance.transfer(second, writer)
        self.assertEqual(writer.data, b"")
        self.assertEqual(instance.bytes, 3)

    async def test_idle_read_has_finite_deadline(self):
        instance = relay.Relay(args("--idle", "0.01"))
        with self.assertRaises(asyncio.TimeoutError):
            await instance.transfer(asyncio.StreamReader(), Writer())

    async def test_connect_timeout_is_failure_not_keepalive_idle(self):
        instance = relay.Relay(args("--mode", "inner"))
        instance.identity = (1, 2)
        with patch.object(relay, "socket_identity", return_value=(1, 2)), patch.object(relay.asyncio, "open_unix_connection", new=AsyncMock(side_effect=asyncio.TimeoutError)):
            await instance.handle(asyncio.StreamReader(), Writer())
        self.assertEqual(instance.failure, "connect-timeout")
        self.assertEqual(instance.idle_closed, 0)
        self.assertEqual(instance.active, 0)

    def test_receipt_contains_counts_and_static_categories_only(self):
        instance = relay.Relay(args())
        instance.fail("byte-bound")
        self.assertEqual(instance.failure, "byte-bound")
        self.assertTrue(instance.stop.is_set())


if __name__ == "__main__":
    unittest.main()
