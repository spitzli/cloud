"""Real Node HTTPS runtime to Bun pinned transport interoperability; no provider access."""
import json
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import tempfile
import unittest

import plugin_installer as installer

ROOT = Path(__file__).resolve().parents[2]
NODE = os.environ.get('WEBDOCK_TEST_NODE') or shutil.which('node')
BUN = os.environ.get('BUN_BINARY') or shutil.which('bun') or 'bun'


class PluginTLSTests(unittest.TestCase):
    def test_generated_runtime_tls_supports_verified_bun_to_node_health(self):
        self.assertTrue(callable(getattr(installer, 'generate_runtime_tls', None)), 'Runtime TLS generator must be independently exercised')
        if not NODE or not Path(BUN).exists():
            self.skipTest('Node and Bun are required for the real TLS regression')
        version = subprocess.check_output([NODE, '--version'], text=True).strip()
        if int(version.split('.')[0][1:]) < 24:
            self.skipTest('Use Node 24+ or WEBDOCK_TEST_NODE for runtime TLS interoperability')
        dns = 'pi-123.plugin-test.svc.cluster.local'
        certificate, key = installer.generate_runtime_tls(dns)
        other_ca, _ = installer.generate_runtime_tls(dns)
        server = r"""
const fs=require('node:fs'),https=require('node:https');
const server=https.createServer({cert:fs.readFileSync(process.argv[1]),key:fs.readFileSync(process.argv[2]),minVersion:'TLSv1.2'},(req,res)=>{
 res.writeHead(req.headers.authorization==='Bearer fixture-runtime-token'?200:401,{'content-type':'application/json'});
 res.end(JSON.stringify({pluginID:'fixture',version:'1.0.0',generation:1,protocolVersion:1}));
});
server.on('tlsClientError',()=>{});
server.listen(0,'127.0.0.1',()=>console.log(server.address().port));
"""
        client = r"""
const {pinnedHTTPTransport}=await import(process.argv[1]);
try {
 const result=await pinnedHTTPTransport({url:new URL(`https://${process.argv[3]}:${process.argv[4]}/healthz`),addresses:['127.0.0.1'],ca:await Bun.file(process.argv[2]).text(),headers:{authorization:'Bearer fixture-runtime-token'},method:'GET'});
 console.log(JSON.stringify(result));
} catch { console.log(JSON.stringify({verified:false}));process.exitCode=1; }
"""
        with tempfile.TemporaryDirectory(prefix='webdock-runtime-tls-test-') as directory:
            folder = Path(directory)
            (folder / 'tls.crt').write_text(certificate)
            (folder / 'tls.key').write_text(key)
            (folder / 'wrong.crt').write_text(other_ca)
            process = subprocess.Popen([NODE, '-e', server, str(folder / 'tls.crt'), str(folder / 'tls.key')], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    self.assertTrue(selector.select(10), 'Node runtime did not listen')
                port = process.stdout.readline().strip()
                self.assertTrue(port.isdigit(), 'Node runtime failed before listening')
                def request(ca, hostname):
                    return subprocess.run([BUN, '--no-env-file', '-e', client, str(ROOT / 'packages/core/src/plugin-http.ts'), str(ca), hostname, port], cwd=ROOT, capture_output=True, text=True, timeout=15)
                health = request(folder / 'tls.crt', dns)
                self.assertEqual(health.returncode, 0, 'Verified Bun-to-Node TLS failed: ' + health.stdout)
                self.assertEqual(json.loads(health.stdout), {'status': 200, 'body': {'pluginID': 'fixture', 'version': '1.0.0', 'generation': 1, 'protocolVersion': 1}})
                self.assertNotEqual(request(folder / 'tls.crt', 'different.plugin-test.svc.cluster.local').returncode, 0, 'Wrong DNS identity must be rejected')
                self.assertNotEqual(request(folder / 'wrong.crt', dns).returncode, 0, 'Untrusted issuer must be rejected')
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait()
                process.stdout.close(); process.stderr.close()


if __name__ == '__main__':
    unittest.main()
