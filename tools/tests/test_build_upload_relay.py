import asyncio
import importlib.util
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location('build_upload_relay',Path(__file__).with_name('build-upload-relay.py'))
relay=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(relay)


class Ownership(unittest.TestCase):
    def test_only_scoped_owner_fixed_endpoint_and_bounded_unit(self):
        self.assertEqual(relay.LISTEN,('167.86.75.144',19444))
        self.assertEqual(relay.UPSTREAM,('127.0.0.1',19445))
        for bad in ['other','native-../victim','native-a;stop','native-A','native-'+('a'*49)]:
            with self.assertRaises(ValueError):relay.owner(bad)
        args=relay.unit_command('native-20261030-full1')
        for item in ['MemoryMax=64M','MemorySwapMax=0','TasksMax=4','LimitNOFILE=32','RuntimeMaxSec=7200','StandardOutput=null','StandardError=null','NoNewPrivileges=yes']:
            self.assertIn('--property='+item,args)
        self.assertEqual(args[-6:],['/usr/bin/python3','-B','/root/webdock-build-upload-relay-native-20261030-full1/relay.py','serve','--owner','native-20261030-full1'])
        self.assertNotIn('server.key',' '.join(args))

    def test_private_files_reject_public_modes_symlinks_and_hardlinks(self):
        with tempfile.TemporaryDirectory() as work:
            path=Path(work)/'file';relay.write_new(path,b'private')
            self.assertEqual(relay.private_read(path),b'private')
            path.chmod(0o644)
            with self.assertRaises(ValueError):relay.private_read(path)
            path.chmod(0o600)
            linked=Path(work)/'linked';linked.symlink_to(path)
            with self.assertRaises(OSError):relay.private_read(linked)
            linked.unlink();os.link(path,linked)
            with self.assertRaises(ValueError):relay.private_read(path)

    def test_pid_starttime_fence_rejects_changed_process_before_any_kill(self):
        current=relay.proc_start(os.getpid())
        with self.assertRaises(ValueError):
            relay.process_identity(os.getpid(),Path('/not-an-owned-relay'),{'startTimeTicks':current+1})

    def test_stop_cannot_signal_a_unit_without_matching_pid_custody(self):
        with patch.object(relay,'guard'),patch.object(relay,'verified_files',return_value=(Path('/unreachable'),{'unit':'owned.service'})),patch.object(relay,'verified_status',side_effect=ValueError('relay_process_identity_changed')),patch.object(relay,'command') as command:
            with self.assertRaises(ValueError):relay.manage('stop','native-20261030-full1','expected')
            command.assert_not_called()

    def test_stop_removes_only_exact_private_files_after_verified_unit_stops(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)/'owned';root.mkdir(mode=0o700)
            for name in ['relay.py','server.crt','server.key','ownership.json','process.json']:relay.write_new(root/name,b'owned')
            observed={'pid':12345,'startTimeTicks':123}
            with patch.object(relay,'guard'),patch.object(relay,'verified_files',return_value=(root,{'unit':'owned.service'})),patch.object(relay,'verified_status',return_value=observed),patch.object(relay,'proc_start',side_effect=FileNotFoundError),patch.object(relay,'command',return_value='') as command:
                self.assertTrue(relay.manage('stop','native-20261030-full1','expected')['exactPrivateFilesRemoved'])
                command.assert_called_once_with(['systemctl','stop','owned.service'])
            self.assertFalse(root.exists())

    def test_unknown_private_file_prevents_broad_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            for name in ['relay.py','server.crt','server.key','ownership.json','process.json','unowned']:relay.write_new(root/name,b'owned')
            with patch.object(relay,'guard'),patch.object(relay,'verified_files',return_value=(root,{'unit':'owned.service'})),patch.object(relay,'verified_status',return_value={'pid':12345,'startTimeTicks':123}),patch.object(relay,'proc_start',side_effect=FileNotFoundError),patch.object(relay,'command',return_value=''):
                with self.assertRaises(ValueError):relay.manage('stop','native-20261030-full1','expected')
            self.assertEqual(len(list(root.iterdir())),6)


class Duplex(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.directory=tempfile.TemporaryDirectory();root=Path(self.directory.name)
        self.key=root/'key.pem';self.cert=root/'cert.pem'
        subprocess.run(['openssl','req','-new','-x509','-newkey','rsa:2048','-nodes','-keyout',str(self.key),'-out',str(self.cert),'-days','1','-subj','/CN=owned-relay-test','-addext','subjectAltName=IP:127.0.0.1'],check=True,capture_output=True)
        self.context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);self.context.load_cert_chain(self.cert,self.key)
        self.client_context=ssl.create_default_context(cafile=str(self.cert));self.tasks=[];self.servers=[]

    async def asyncTearDown(self):
        for server in self.servers:server.close();await server.wait_closed()
        for task in self.tasks:task.cancel()
        await asyncio.gather(*self.tasks,return_exceptions=True)
        self.directory.cleanup()

    async def connect(self,handler,lifetime=2):
        upstream=await asyncio.start_server(handler,'127.0.0.1',0,limit=relay.CHUNK);self.servers.append(upstream)
        target=upstream.sockets[0].getsockname()
        listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen(1);listener.setblocking(False)
        address=listener.getsockname()
        async def accepted():
            try:
                connection,_=await asyncio.get_running_loop().sock_accept(listener)
                with patch.object(relay,'UPSTREAM',target),patch.object(relay,'LIFETIME',lifetime):
                    await relay.relay_socket(connection,self.context)
            finally:listener.close()
        task=asyncio.create_task(accepted());self.tasks.append(task)
        reader,writer=await asyncio.open_connection(*address,ssl=self.client_context,server_hostname='127.0.0.1')
        return reader,writer,task

    async def test_tls_duplex_streams_chunks_without_collecting_a_whole_body(self):
        largest=0
        async def echo(reader,writer):
            nonlocal largest
            try:
                while data:=await reader.read(relay.CHUNK):
                    largest=max(largest,len(data));writer.write(data);await writer.drain()
            finally:writer.close();await writer.wait_closed()
        reader,writer,task=await self.connect(echo)
        payload=b'owned-byte-stream'*32768
        async def send():
            for offset in range(0,len(payload),relay.CHUNK):
                writer.write(payload[offset:offset+relay.CHUNK]);await writer.drain()
        sending=asyncio.create_task(send());self.tasks.append(sending)
        self.assertEqual(await asyncio.wait_for(reader.readexactly(len(payload)),2),payload)
        await sending;self.assertLessEqual(largest,relay.CHUNK)
        writer.close();await writer.wait_closed();await task

    async def test_upstream_early_reply_is_forwarded_while_client_body_is_absent(self):
        reply=b'HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'
        async def reject(reader,writer):
            writer.write(reply);await writer.drain();writer.close();await writer.wait_closed()
        reader,writer,task=await self.connect(reject)
        self.assertEqual(await asyncio.wait_for(reader.read(),1),reply)
        writer.close();await writer.wait_closed();await task

    async def test_absolute_deadline_closes_idle_bidirectional_connection(self):
        async def idle(reader,writer):
            await reader.read();writer.close();await writer.wait_closed()
        reader,writer,task=await self.connect(idle,lifetime=0.1)
        self.assertEqual(await asyncio.wait_for(reader.read(1),1),b'')
        writer.close();await writer.wait_closed();await task


if __name__=='__main__':unittest.main()
