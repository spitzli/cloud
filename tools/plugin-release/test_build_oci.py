import importlib.util
import json
import os
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('build_oci',Path(__file__).with_name('build_oci.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class BuildOCI(unittest.TestCase):
    def test_component_dockerfile_stays_inside_repository(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td); component=root/'apps/cms-instance';component.mkdir(parents=True)
            (component/'Dockerfile').write_text('FROM scratch\n')
            self.assertEqual(module.selected_dockerfile(root,'apps/cms-instance/Dockerfile'),'apps/cms-instance/Dockerfile')
            for value in ['/tmp/Dockerfile','../Dockerfile','missing/Dockerfile','apps/cms-instance/.env']:
                with self.assertRaises(ValueError):module.selected_dockerfile(root,value)
            (root/'alias').symlink_to(component,target_is_directory=True)
            with self.assertRaises(ValueError):module.selected_dockerfile(root,'alias/Dockerfile')

    def test_metadata_accepts_actual_digest_only(self):
        digest='sha256:'+'1234567890abcdef'*4
        self.assertEqual(module.metadata_digest({'containerimage.digest':digest}),digest)
        for value in [{},{'containerimage.digest':'sha256:'+'0'*64},{'containerimage.digest':'latest'},{'containerimage.digest':'sha256:'+'a'*64}]:
            with self.assertRaises(ValueError):module.metadata_digest(value)

    def test_socket_requires_current_uid_runtime_directory_and_regular_socket(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);path=root/'buildkit.sock'
            with socket.socket(socket.AF_UNIX) as listener:
                listener.bind(str(path))
                self.assertEqual(module.validate_socket('unix://'+str(path),os.getuid(),root),str(path))
                with self.assertRaises(ValueError):module.validate_socket('tcp://host:1234',os.getuid(),root)
                alias=root/'alias';alias.symlink_to(path)
                with self.assertRaises(ValueError):module.validate_socket('unix://'+str(alias),os.getuid(),root)
            plain=root/'plain';plain.write_text('')
            with self.assertRaises(ValueError):module.validate_socket('unix://'+str(plain),os.getuid(),root)
            with self.assertRaises(ValueError):module.validate_socket('unix://'+str(path),os.getuid()+1,root)

    def test_auth_is_private_temporary_and_removed_after_success_or_failure(self):
        for fail in [False,True]:
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as td:
                root=Path(td);context=root/'context';context.mkdir();(context/'Dockerfile').write_text('FROM scratch\n')
                def fake_run(args,**kwargs):
                    if '--version' in args:return module.subprocess.CompletedProcess(args,0,'buildctl github.com/moby/buildkit v0.34.1 test','')
                    self.assertIn('label:org.opencontainers.image.source=https://github.com/spitzli/cloud-plugin-fixture-source',args)
                    auth=Path(kwargs['env']['DOCKER_CONFIG'])/'config.json'
                    self.assertEqual(auth.stat().st_mode & 0o777,0o600)
                    config=json.loads(auth.read_text());self.assertIn('ghcr.io',config['auths'])
                    self.assertNotIn('GH_TOKEN',kwargs['env'])
                    if fail:
                        import base64
                        credential='ghs_testcredential'
                        encoded=base64.b64encode(('fixture:'+credential).encode()).decode()
                        kwargs['stdout'].write(b'A'*(module.MAX_FAILURE_LOG_BYTES+10000)+b'\n'+credential.encode()+b' '+encoded.encode()+b'\n::warning::source-controlled text\n')
                        return module.subprocess.CompletedProcess(args,1,'','')
                    meta=Path(args[args.index('--metadata-file')+1]);meta.write_text(json.dumps({'containerimage.digest':'sha256:'+'1234567890abcdef'*4}))
                    return module.subprocess.CompletedProcess(args,0,'','')
                config={'HOME':str(root),'IMAGE_REPOSITORY':'ghcr.io/spitzli/cloud-plugin-fixture','GITHUB_REPOSITORY':'spitzli/cloud-plugin-fixture-source','GITHUB_SHA':'b'*40,'GITHUB_ACTOR':'fixture','SPITZLI_BUILDKIT_ADDR':'unix:///run/user/1000/buildkit.sock','SPITZLI_BUILDCTL_SHA256':'c'*64}
                with patch.object(module,'qualified_buildctl',return_value='/qualified/buildctl'),patch.object(module,'validate_socket',return_value='/run/user/1000/buildkit.sock'),patch.object(module.subprocess,'run',side_effect=fake_run):
                    if fail:
                        with self.assertRaises(module.BuildFailed) as caught:module.build_image(context,root,config,'ghs_testcredential')
                        retained=caught.exception.private_log
                        raw=retained.read_bytes()
                        self.assertEqual(retained.parent,root/'ci-failures')
                        self.assertEqual(retained.parent.stat().st_mode & 0o777,0o700)
                        self.assertEqual(retained.stat().st_mode & 0o777,0o600)
                        self.assertLessEqual(len(raw),module.MAX_FAILURE_LOG_BYTES)
                        self.assertNotIn(b'ghs_testcredential',raw)
                        self.assertNotIn(b'Zml4dHVyZTpnaHNfdGVzdGNyZWRlbnRpYWw=',raw)
                        self.assertIn(b'[REDACTED]',raw)
                        self.assertIn(b'::warning::source-controlled text',raw)
                    else:self.assertTrue(module.build_image(context,root,config,'ghs_testcredential').startswith('sha256:'))
                self.assertFalse([p for p in root.glob('plugin-build-*') if p.is_dir()])
                if not fail:self.assertFalse(list(root.glob('plugin-build-failure-*')))


class CheckTarget(unittest.TestCase):
    def test_check_target_cannot_push_or_report_image_digest(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);context=root/'context';context.mkdir();(context/'Dockerfile').write_text('FROM scratch AS check\n')
            config={'IMAGE_REPOSITORY':'ghcr.io/spitzli/cloud-plugin-fixture','GITHUB_SHA':'b'*40,'GITHUB_ACTOR':'fixture','SPITZLI_BUILDKIT_ADDR':'unix:///run/user/1000/buildkit.sock'}
            def run(args,**kwargs):
                self.assertIn('target=check',args)
                self.assertNotIn('--output',args)
                self.assertFalse(any('push=true' in value for value in args))
                self.assertNotIn('--metadata-file',args)
                auth=json.loads((Path(kwargs['env']['DOCKER_CONFIG'])/'config.json').read_text())
                self.assertEqual(auth,{'auths':{}})
                return module.subprocess.CompletedProcess(args,0,'','')
            with patch.object(module,'qualified_buildctl',return_value='/qualified/buildctl'),patch.object(module,'validate_socket',return_value='/run/user/1000/buildkit.sock'),patch.object(module.subprocess,'run',side_effect=run):
                self.assertIsNone(module.build_image(context,root,config,'',check=True))
            self.assertFalse(list(root.glob('plugin-build-*')))


class BinaryPin(unittest.TestCase):
    def test_client_checksum_and_version_are_both_required(self):
        import hashlib
        with tempfile.TemporaryDirectory() as td:
            path=Path(td)/'buildctl';path.write_bytes(b'qualified-binary-fixture');path.chmod(0o700)
            config={'SPITZLI_BUILDCTL_SHA256':hashlib.sha256(path.read_bytes()).hexdigest()}
            with patch.object(module.shutil,'which',return_value=str(path)),patch.object(module.subprocess,'run',return_value=module.subprocess.CompletedProcess([],0,'buildctl github.com/moby/buildkit v0.34.1 fixture','')):
                self.assertEqual(module.qualified_buildctl(config),str(path))
                with self.assertRaises(ValueError):module.qualified_buildctl({'SPITZLI_BUILDCTL_SHA256':'0'*64})
            with patch.object(module.shutil,'which',return_value=str(path)),patch.object(module.subprocess,'run',return_value=module.subprocess.CompletedProcess([],0,'buildctl github.com/moby/buildkit v0.30.0 fixture','')):
                with self.assertRaises(ValueError):module.qualified_buildctl(config)


class BootstrapArchive(unittest.TestCase):
    def test_bootstrap_exports_only_backend_archive_without_push_or_attestation(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);context=root/'context';context.mkdir()
            dockerfile=context/'deploy/docker';dockerfile.mkdir(parents=True)
            (dockerfile/'backend.Dockerfile').write_text('FROM scratch AS backend\n')
            config={'HOME':str(root),'IMAGE_REPOSITORY':'ghcr.io/spitzli/webdock','GITHUB_SHA':'b'*40,'GITHUB_ACTOR':'fixture','SPITZLI_BUILDKIT_ADDR':'unix:///run/user/1000/buildkit.sock'}
            def run(args,**kwargs):
                self.assertIn('filename=deploy/docker/backend.Dockerfile',args)
                self.assertIn('target=backend',args)
                self.assertIn('build-arg:SOURCE_REVISION='+'b'*40,args)
                self.assertEqual(args[args.index('--output')+1],'type=oci,dest='+str(root/'webdock-backend.oci.tar'))
                self.assertFalse(any('push=true' in value or 'attest:' in value for value in args))
                self.assertNotIn('--metadata-file',args)
                self.assertEqual(json.loads((Path(kwargs['env']['DOCKER_CONFIG'])/'config.json').read_text()),{'auths':{}})
                (root/'webdock-backend.oci.tar').write_bytes(b'archive-fixture')
                return module.subprocess.CompletedProcess(args,0,'','')
            with patch.object(module,'qualified_buildctl',return_value='/qualified/buildctl'),patch.object(module,'validate_socket',return_value='/run/user/1000/buildkit.sock'),patch.object(module.subprocess,'run',side_effect=run):
                self.assertIsNone(module.build_image(context,root,config,'',bootstrap=True))
                with self.assertRaises(ValueError):module.build_image(context,root,config,'',bootstrap=True)
                with self.assertRaises(ValueError):module.build_image(context,root,config,'ghs_testcredential',bootstrap=True)
                with self.assertRaises(ValueError):module.build_image(context,root,config,'',check=True,bootstrap=True)
            self.assertEqual((root/'webdock-backend.oci.tar').read_bytes(),b'archive-fixture')


class PersistentPrivateLogs(unittest.TestCase):
    def test_retains_at_most_ten_bounded_files_and_rejects_symlink_escape(self):
        with tempfile.TemporaryDirectory() as td:
            home=Path(td);source=home/'source.log';source.write_bytes(b'line\n'*600000)
            for _ in range(12):module.retain_failure_log(source,home,[])
            files=list((home/'ci-failures').glob('plugin-build-failure-*.log'))
            self.assertEqual(len(files),10)
            self.assertLessEqual(sum(path.stat().st_size for path in files),20*1024*1024)
            self.assertTrue(all(path.stat().st_mode & 0o777 == 0o600 for path in files))
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);home=root/'home';home.mkdir();outside=root/'outside';outside.mkdir()
            (home/'ci-failures').symlink_to(outside,target_is_directory=True)
            with self.assertRaises(ValueError):module.failure_directory(home)
            self.assertEqual(list(outside.iterdir()),[])
            alias=root/'alias';alias.symlink_to(home,target_is_directory=True)
            with self.assertRaises(ValueError):module.failure_directory(alias)


class BlacksmithBuildx(unittest.TestCase):
    def config(self, root):
        docker_root=root/'provider-docker';(docker_root/'buildx').mkdir(parents=True)
        return {'HOME':str(root),'PATH':'/usr/bin','DOCKER_CONFIG':str(docker_root),'GITHUB_ACTIONS':'true',
                'BLACKSMITH_REGION':'us-west','SPITZLI_BLACKSMITH_REGION':'us-west','SPITZLI_OCI_BACKEND':'blacksmith-buildx',
                'IMAGE_REPOSITORY':'ghcr.io/spitzli/cloud-plugin-fixture','GITHUB_REPOSITORY':'spitzli/cloud-plugin-fixture',
                'GITHUB_SHA':'b'*40,'GITHUB_ACTOR':'fixture'}

    def test_provider_builder_requires_exact_remote_loopback_selection(self):
        # Source-derived rendering fixture, not an observed CI qualification result.
        valid=Path(__file__).with_name('fixtures').joinpath('buildx-v0.23.0-inspect.txt').read_text()
        self.assertEqual(module.validate_blacksmith_builder(valid),'blacksmith-miw0u1uo')
        for changed in [valid.replace('blacksmith-miw0u1uo','local'),valid.replace('remote','docker-container'),
                        valid.replace('127.0.0.1','10.0.0.1'),valid.replace('running','inactive'),
                        valid+'Endpoint: tcp://127.0.0.1:1234\n',valid+'Error: unavailable\n',
                        valid+'Flags: --oci-worker-no-process-sandbox\n','x'*131073]:
            with self.subTest(changed=changed[:100]),self.assertRaises(ValueError):module.validate_blacksmith_builder(changed)

    def test_preexisting_daemon_or_unknown_provider_fails_before_setup(self):
        with tempfile.TemporaryDirectory() as td:
            config=self.config(Path(td))
            for code, allowed in [(1,True),(0,False),(2,False)]:
                with patch.object(module.subprocess,'run',return_value=module.subprocess.CompletedProcess([],code,'','')):
                    if allowed:module.require_fresh_blacksmith_builder(config)
                    else:
                        with self.assertRaises(ValueError):module.require_fresh_blacksmith_builder(config)
            for field,value in [('GITHUB_ACTIONS','false'),('BLACKSMITH_REGION',''),('SPITZLI_BLACKSMITH_REGION','eu-central')]:
                with self.subTest(field=field),self.assertRaises(ValueError):module.require_fresh_blacksmith_builder({**config,field:value})

    def test_inspection_keeps_provider_config_and_does_not_forward_job_credentials(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);config={**self.config(root),'GH_TOKEN':'must-not-forward','DOCKER_HOST':'tcp://unreviewed:1234'}
            valid='Name: blacksmith-abc123\nDriver: remote\nNodes:\nName: blacksmith-abc1230\nEndpoint: tcp://127.0.0.1:1234\nStatus: running\n'
            def run(args,**kwargs):
                self.assertEqual(kwargs['env']['DOCKER_CONFIG'],str(root/'provider-docker'))
                self.assertEqual(kwargs['env']['BUILDX_CONFIG'],str(root/'provider-docker/buildx'))
                self.assertNotIn('GH_TOKEN',kwargs['env']);self.assertNotIn('DOCKER_HOST',kwargs['env'])
                return module.subprocess.CompletedProcess(args,0,'github.com/docker/buildx v0.23.0 test' if args[-1]=='version' else valid,'')
            with patch.object(module.shutil,'which',return_value='/qualified/docker'),patch.object(module.subprocess,'run',side_effect=run):
                self.assertEqual(module.qualified_blacksmith_builder(config),('/qualified/docker','blacksmith-abc123',str(root/'provider-docker')))
                with self.assertRaises(ValueError):module.qualified_blacksmith_builder({**config,'BUILDX_CONFIG':'/unreviewed/config'})

    def test_buildx_modes_preserve_private_auth_source_and_export_contracts(self):
        for mode in ['push','check','bootstrap','failure']:
            with self.subTest(mode=mode),tempfile.TemporaryDirectory() as td:
                root=Path(td);context=root/'context';context.mkdir();(context/'Dockerfile').write_text('FROM scratch AS check\n')
                backend=context/'deploy/docker';backend.mkdir(parents=True);(backend/'backend.Dockerfile').write_text('FROM scratch AS backend\n')
                config=self.config(root);token='ghs_testcredential' if mode in ['push','failure'] else ''
                def run(args,**kwargs):
                    environment=kwargs['env'];private=Path(environment['DOCKER_CONFIG'])
                    auth=json.loads((private/'config.json').read_text())
                    self.assertNotIn('GH_TOKEN',environment)
                    self.assertEqual(environment['BUILDX_CONFIG'],str(root/'provider-docker/buildx'))
                    self.assertEqual(auth['cliPluginsExtraDirs'],[str(root/'provider-docker/cli-plugins')])
                    self.assertEqual((private/'config.json').stat().st_mode & 0o777,0o600)
                    self.assertEqual(bool(auth['auths']),bool(token))
                    self.assertEqual(args[:5],['/qualified/docker','buildx','build','--builder','blacksmith-abc123'])
                    self.assertIn('org.opencontainers.image.revision='+'b'*40,args)
                    self.assertIn('org.opencontainers.image.source=https://github.com/spitzli/cloud-plugin-fixture',args)
                    self.assertFalse(any(value in args for value in ['--allow','--network','--secret','--ssh','--load']))
                    self.assertEqual(args[-1],str(context))
                    if mode=='check':
                        self.assertIn('check',args);self.assertNotIn('--output',args);self.assertNotIn('--metadata-file',args)
                    elif mode=='bootstrap':
                        self.assertIn('--provenance=false',args);self.assertIn('--sbom=false',args)
                        self.assertIn('SOURCE_REVISION='+'b'*40,args)
                        self.assertEqual(args[args.index('--output')+1],'type=oci,dest='+str(root/'webdock-backend.oci.tar'))
                        (root/'webdock-backend.oci.tar').write_bytes(b'archive')
                    else:
                        self.assertIn('type=image,name=ghcr.io/spitzli/cloud-plugin-fixture:'+'b'*40+',push=true',args)
                        self.assertIn('--provenance=mode=max',args)
                        if mode=='failure':
                            kwargs['stdout'].write(token.encode()+b'\n::warning::untrusted\n')
                            return module.subprocess.CompletedProcess(args,1,'','')
                        Path(args[args.index('--metadata-file')+1]).write_text(json.dumps({'containerimage.digest':'sha256:'+'1234567890abcdef'*4}))
                    return module.subprocess.CompletedProcess(args,0,'','')
                selected=('/qualified/docker','blacksmith-abc123',str(root/'provider-docker'))
                with patch.object(module,'qualified_blacksmith_builder',return_value=selected),patch.object(module,'validate_socket') as socket_check,patch.object(module,'qualified_buildctl') as rootless,patch.object(module.subprocess,'run',side_effect=run):
                    if mode=='failure':
                        with self.assertRaises(module.BuildFailed) as caught:module.build_image(context,root,config,token)
                        self.assertNotIn(token.encode(),caught.exception.private_log.read_bytes())
                    else:
                        digest=module.build_image(context,root,config,token,check=mode=='check',bootstrap=mode=='bootstrap')
                        self.assertEqual(digest,'sha256:'+'1234567890abcdef'*4 if mode=='push' else None)
                    socket_check.assert_not_called();rootless.assert_not_called()
                self.assertFalse(list(root.glob('plugin-build-*')))


if __name__ == "__main__":
    unittest.main()
