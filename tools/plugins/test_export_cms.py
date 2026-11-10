import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('export_cms', Path(__file__).with_name('export_cms.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ExportCMS(unittest.TestCase):
    def fixture(self, root):
        for name in module.PACKAGES:
            package = root / 'packages' / name
            (package / 'src').mkdir(parents=True)
            (package / 'src/index.ts').write_text('export const fixture = true;\n')
            (package / 'package.json').write_text(json.dumps({'name':'@spitzli/'+name,'version':'0.1.0','type':'module'}))
        for relative in module.APPLICATIONS:
            package = root / relative
            (package / 'src').mkdir(parents=True)
            (package / 'src/index.ts').write_text('export const fixture = true;\n')
            (package / 'package.json').write_text(json.dumps({'name':'@spitzli/'+package.name,'version':'0.1.0','type':'module'}))
        for name in module.ROOT_FILES:
            (root / name).write_text('Fixture license\n')
        (root / 'package.json').write_text(json.dumps({'patchedDependencies':{},'overrides':{}}))

    def test_export_contains_only_source_and_declared_workspace(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)/'source'; root.mkdir(); self.fixture(root)
            (root/'apps/cms-instance/.env.production').write_text('PRIVATE=not-exported')
            (root/'packages/cms-ui/src/.env.local').write_text('PRIVATE=not-exported')
            (root/'packages/cms-ui/node_modules').mkdir()
            (root/'packages/cms-ui/node_modules/private.json').write_text('{}')
            (root/'Core-secret.json').write_text('{}')
            catalog=root/'extensions/managed-cms/catalog'
            (catalog/'assets').mkdir(parents=True)
            (catalog/'screenshots').mkdir()
            (catalog/'README.md').write_text('Catalog guide\n')
            (catalog/'assets/icon.svg').write_text('<svg/>')
            (catalog/'screenshots/editor.png').write_bytes(b'PNG fixture')
            target=Path(td)/'export'
            result=module.export_cms(root,target)
            self.assertEqual(result['protocolVersion'],1)
            self.assertFalse((target/'Core-secret.json').exists())
            self.assertFalse(list(target.rglob('.env.production')))
            self.assertFalse(list(target.rglob('node_modules')))
            pkg=json.loads((target/'package.json').read_text())
            self.assertEqual(set(pkg['workspaces']),set(module.APPLICATIONS + ['packages/'+n for n in module.PACKAGES]))
            self.assertFalse((target/'bun.lock').exists())
            self.assertTrue((target/'extensions/managed-cms/Dockerfile').is_file())
            self.assertTrue((target/'apps/cms-instance/Dockerfile').is_file())
            self.assertTrue((target/'export-lock.json').is_file())
            self.assertEqual((target/'README.md').read_text(),'Catalog guide\n')
            self.assertEqual((target/'assets/icon.svg').read_text(),'<svg/>')
            self.assertEqual((target/'screenshots/editor.png').read_bytes(),b'PNG fixture')
            self.assertIn('screenshots/editor.png',result['files'])

    def test_symlink_and_secret_reject_without_partial_destination(self):
        for kind in ['symlink','secret']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as td:
                root=Path(td)/'source';root.mkdir();self.fixture(root)
                file=root/'packages/cms-ui/src/unsafe.ts'
                if kind=='symlink':file.symlink_to('/etc/passwd')
                else:file.write_text('-----BEGIN PRIVATE KEY-----\nsecret\n')
                target=Path(td)/'export'
                with self.assertRaises(ValueError):module.export_cms(root,target)
                self.assertFalse(target.exists())

    def test_unknown_workspace_dependency_and_existing_destination_reject(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)/'source';root.mkdir();self.fixture(root)
            package=root/'packages/cms-ui/package.json';pkg=json.loads(package.read_text());pkg['dependencies']={'@spitzli/core':'*'};package.write_text(json.dumps(pkg))
            with self.assertRaises(ValueError):module.export_cms(root,Path(td)/'export')
            existing=Path(td)/'existing';existing.mkdir()
            with self.assertRaises(ValueError):module.export_cms(root,existing)

if __name__=='__main__': unittest.main()
