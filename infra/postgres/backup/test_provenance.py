"""Security contract tests: mutation must invalidate a real signed local bundle."""
import json
import pathlib
import subprocess
import tempfile
import tarfile
import io
import unittest

import provenance as p


class ProvenanceTests(unittest.TestCase):
    def test_mutable_base_is_rejected(self):
        for ref in ('debian:trixie-slim', 'debian@sha256:short', 'debian@sha256:' + 'x' * 64):
            with self.assertRaises(ValueError):
                p.immutable(ref)

    def test_path_traversal_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            for name in ('../outside', '/etc/passwd', 'sub/../../outside'):
                with self.assertRaises(ValueError):
                    p.material_path(pathlib.Path(d), name)

    def test_real_signature_and_material_tampering(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / 'source.tar').write_bytes(b'reviewed source')
            p.new_fixture_key(root)
            statement = p.statement(root, {'source.tar': p.sha(root / 'source.tar')}, [])
            p.sign(root, statement)
            pin = p.sha(root / 'fixture-public.pem')
            self.assertEqual(p.verify_envelope(root, pin)['predicate']['qualification'], 'LOCAL FIXTURE ONLY')
            (root / 'source.tar').write_bytes(b'attacker source')
            with self.assertRaises(ValueError):
                p.verify_envelope(root, pin)
            (root / 'source.tar').write_bytes(b'reviewed source')
            doc = json.loads((root / 'provenance.json').read_text())
            doc['predicate']['source']['commit'] = '0' * 40
            (root / 'provenance.json').write_text(json.dumps(doc))
            with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                p.verify_envelope(root, pin)

    def test_foreign_public_pin_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            p.new_fixture_key(root)
            p.sign(root, p.statement(root, {}, []))
            with self.assertRaises(ValueError):
                p.verify_envelope(root, '0' * 64)

    def test_valid_signature_cannot_authorize_a_different_source(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            p.new_fixture_key(root)
            doc = p.statement(root, {}, [])
            doc['predicate']['source']['commit'] = '0' * 40
            p.sign(root, doc)
            with self.assertRaises(ValueError):
                p.verify_envelope(root, p.sha(root / 'fixture-public.pem'))

    def test_changed_extracted_build_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / 'src').mkdir()
            (root / 'src' / 'main.go').write_bytes(b'package main\n')
            with tarfile.open(root / 'source.tar', 'w') as archive:
                info = tarfile.TarInfo('main.go')
                info.size = 13
                archive.addfile(info, io.BytesIO(b'package main\n'))
            p.verify_extracted_source(root)
            (root / 'src' / 'main.go').write_bytes(b'package evil\n')
            with self.assertRaises(ValueError):
                p.verify_extracted_source(root)
            (root / 'src' / 'main.go').write_bytes(b'package main\n')
            (root / 'src' / 'injected.go').write_text('malicious source')
            with self.assertRaises(ValueError):
                p.verify_extracted_source(root)

    def test_oci_digest_tampering_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            layer = root / 'rootfs.tar'
            layer.write_bytes(b'test layer content')
            dest = root / 'image'
            digest = p.make_oci(layer, dest, 'localhost/webdock-barman-plugin-local:test', {})
            self.assertEqual(p.verify_oci(dest), digest)
            blob = next((dest / 'blobs' / 'sha256').iterdir())
            blob.write_bytes(b'tamper')
            with self.assertRaises(ValueError):
                p.verify_oci(dest)

    def test_only_directory_symlink_trailing_slash_is_equivalent(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            (root / 'src').mkdir()
            (root / 'src/config').mkdir()
            (root / 'src/link').symlink_to('config')
            with tarfile.open(root / 'source.tar', 'w') as archive:
                info = tarfile.TarInfo('link')
                info.type = tarfile.SYMTYPE
                info.linkname = 'config/'
                archive.addfile(info)
            p.verify_extracted_source(root)
            (root / 'src/link').unlink()
            (root / 'src/link').symlink_to('other')
            with self.assertRaises(ValueError):
                p.verify_extracted_source(root)

    def test_signed_image_claim_without_build_evidence_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            p.new_fixture_key(root)
            subjects = [{'name': 'localhost/webdock-barman-' + c + '-local:test',
                         'digest': {'sha256': '0' * 64}} for c in ('plugin', 'sidecar')]
            p.sign(root, p.statement(root, {}, subjects))
            with self.assertRaises(ValueError):
                p.verify_envelope(root, p.sha(root / 'fixture-public.pem'), require_images=True)

    def test_valid_image_digest_cannot_hide_substituted_executable(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            layer = root / 'layer.tar'
            with tarfile.open(layer, 'w') as archive:
                info = tarfile.TarInfo('manager')
                info.size = 8
                archive.addfile(info, io.BytesIO(b'attacker'))
            dest = root / 'image'
            p.make_oci(layer, dest, 'localhost/webdock-barman-plugin-local:test', {})
            p.verify_oci(dest)
            with self.assertRaises(ValueError):
                p.verify_image_contents(dest, {'manager': {'sha256': '0' * 64}})

    def test_containerd_and_legacy_image_ids_are_bound_to_packaged_content(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            layer = root / 'layer.tar'
            layer.write_bytes(b'layer')
            dest = root / 'image'
            digest = p.make_oci(layer, dest, 'localhost/webdock-barman-plugin-local:test', {})
            manifest = json.loads((dest / 'blobs/sha256' / digest[7:]).read_text())
            cfgid = manifest['config']['digest']
            cfg = json.loads((dest / 'blobs/sha256' / cfgid[7:]).read_text())
            observed = {'Id': digest, 'Descriptor': {'digest': digest}, 'Config': cfg['config'],
                        'Architecture': 'amd64', 'Os': 'linux', 'RootFS': {'Layers': cfg['rootfs']['diff_ids']}}
            p.verify_imported_image(observed, dest)
            observed['Id'] = cfgid
            observed.pop('Descriptor')
            p.verify_imported_image(observed, dest)
            observed['Id'] = 'sha256:' + '0' * 64
            with self.assertRaises(ValueError):
                p.verify_imported_image(observed, dest)
            observed['Id'] = digest
            observed['RootFS']['Layers'] = ['sha256:' + '0' * 64]
            with self.assertRaises(ValueError):
                p.verify_imported_image(observed, dest)

    def test_preparation_cannot_pass_image_gate(self):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            p.new_fixture_key(root)
            p.sign(root, p.statement(root, {}, []))
            with self.assertRaises(ValueError):
                p.verify_envelope(root, p.sha(root / 'fixture-public.pem'), require_images=True)


if __name__ == '__main__':
    unittest.main()
