import base64
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile
import plugin_source
import worker

path=Path(__file__).resolve().parents[2]/'tools/plugins/github-actions/plugin_build.py'
script=None
if path.exists():
    spec=importlib.util.spec_from_file_location('plugin_build',path);script=importlib.util.module_from_spec(spec);spec.loader.exec_module(script)

class PluginBuildTests(unittest.TestCase):
    def setUp(self):self.assertIsNotNone(script,'trusted build helper missing')
    def archive(self):
        out=io.BytesIO()
        with zipfile.ZipFile(out,'w') as z:z.writestr('tabularium.json','{}');z.writestr('Dockerfile','FROM scratch\n')
        return out.getvalue()
    def test_source_oidc_audience_and_hash_are_bound_without_installer_credentials(self):
        data=self.archive();expected=hashlib.sha256(plugin_source.normalize_zip(data)).hexdigest();calls=[]
        env={'ACTIONS_ID_TOKEN_REQUEST_URL':'https://oidc.example/token?x=1','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'request-token'}
        def request(url,headers,body=None):
            calls.append((url,headers,body))
            if url.startswith('https://oidc.example/'):return {'value':'header.payload.signature'}
            return {'data':base64.b64encode(data).decode(),'done':True,'sourceSHA256':expected,'projectID':'456','customerID':'789'}
        with patch.dict(os.environ,env),patch.object(script,'request_json',side_effect=request),patch('builtins.print'):
            archive,metadata=script.acquire('https://api.example/v1/plugin-installer','123','a'*32)
        self.assertEqual(archive,plugin_source.normalize_zip(data))
        self.assertIn('audience=webdock-plugin-build%3A123%3A'+'a'*32,calls[0][0])
        self.assertEqual(calls[1][1]['Authorization'],'Bearer header.payload.signature')
        self.assertEqual(calls[1][2],{'id':'123','nonce':'a'*32,'offset':0})
        self.assertEqual(metadata['sourceSHA256'],expected)
    def test_tampered_source_is_rejected(self):
        data=self.archive()
        def request(url,headers,body=None):
            return {'value':'header.payload.signature'} if body is None else {'data':base64.b64encode(data).decode(),'done':True,'sourceSHA256':'0'*64,'projectID':'456','customerID':'789'}
        with patch.dict(os.environ,{'ACTIONS_ID_TOKEN_REQUEST_URL':'https://oidc.example/token','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'request-token'}),patch.object(script,'request_json',side_effect=request),patch('builtins.print'),self.assertRaises(worker.Rejected):script.acquire('https://api.example/v1/plugin-installer','123','a'*32)
    def test_docker_receives_only_source_context_and_scrubbed_environment(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);source=root/'source';source.mkdir();out=root/'output';out.mkdir();calls=[]
            def run(command,**kwargs):
                calls.append((command,kwargs))
                if 'build' in command:(out/'image.tar').write_bytes(b'fixture')
                return b''
            with patch.dict(os.environ,{'GITHUB_TOKEN':'private-github-token','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'private-oidc','SPITZLI_PLUGIN_INSTALLER_TOKEN':'private-installer'}),patch.object(worker,'run_bounded',side_effect=run):script.build_image(source,out,'123')
            build=next((cmd,opts) for cmd,opts in calls if 'build' in cmd)
            self.assertEqual(build[0][-1],str(source))
            self.assertIn('compression=gzip,force-compression=true', ' '.join(build[0]))
            self.assertNotIn('private-',json.dumps(calls))
            self.assertNotIn('--secret',build[0])
            self.assertNotIn('--ssh',build[0])
            self.assertNotIn('--build-arg',build[0])
    def test_private_build_diagnostics_bound_bytes_and_disable_workflow_commands(self):
        markers=[]
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);source=root/'source';source.mkdir();out=root/'output';out.mkdir()
            failure=worker.BuildFailed('execution_failed')
            failure.logs='Dockerfile:7: compile failed\n::warning::injected\n::add-mask::fake\n::stop-commands::known\n::known::\n'+'€'*worker.MAX_LOG
            def run(command,**kwargs):
                self.assertNotIn('private-',json.dumps(kwargs.get('env',{})))
                if 'build' in command:raise failure
                return b''
            for _ in range(2):
                captured=io.StringIO()
                with patch.dict(os.environ,{'GITHUB_TOKEN':'private-github-token','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'private-oidc','SPITZLI_PLUGIN_INSTALLER_TOKEN':'private-installer'}),patch.object(worker,'run_bounded',side_effect=run),contextlib.redirect_stdout(captured),self.assertRaises(worker.BuildFailed):
                    script.build_image(source,out,'123')
                text=captured.getvalue();first,_,rest=text.partition('\n')
                self.assertRegex(first,r'^::stop-commands::[a-f0-9]{64}$')
                marker=first.removeprefix('::stop-commands::');markers.append(marker)
                self.assertTrue(rest.endswith('\n::'+marker+'::\n'))
                logs=rest.removesuffix('\n::'+marker+'::\n')
                self.assertLessEqual(len(logs.encode()),worker.MAX_LOG)
                self.assertIn('Dockerfile:7: compile failed',logs)
                self.assertIn('::warning::injected',logs)
                self.assertNotIn('private-',text)
                paused=None;interpreted=[]
                for line in text.splitlines():
                    if paused is not None:
                        if line=='::'+paused+'::':paused=None
                    elif line.startswith('::stop-commands::'):paused=line.removeprefix('::stop-commands::')
                    elif line.startswith('::'):interpreted.append(line)
                self.assertIsNone(paused)
                self.assertEqual(interpreted,[])
        self.assertNotEqual(*markers)

    def test_nonbuild_or_raw_provider_errors_are_never_printed(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);source=root/'source';source.mkdir();out=root/'output';out.mkdir()
            for failing_command,error in [('create',worker.BuildFailed('failed','private-provider-token')),('build',RuntimeError('private-provider-token')),('rm',worker.BuildFailed('failed','private-provider-token'))]:
                def run(command,**kwargs):
                    if failing_command in command:raise error
                    if 'build' in command:(out/'image.tar').write_bytes(b'fixture')
                    return b''
                captured=io.StringIO()
                with self.subTest(command=failing_command),patch.object(worker,'run_bounded',side_effect=run),contextlib.redirect_stdout(captured),self.assertRaises(Exception):script.build_image(source,out,'123')
                self.assertEqual(captured.getvalue(),'')


if __name__=='__main__':unittest.main()
