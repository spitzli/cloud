import importlib.util
from pathlib import Path
import unittest

spec=importlib.util.spec_from_file_location('core_tooling',Path(__file__).with_name('core_tooling.py'))
t=importlib.util.module_from_spec(spec);spec.loader.exec_module(t)

class ActualToolIdentity(unittest.TestCase):
    def test_node24_identity_rejects_bun_fallback_and_wrong_versions(self):
        record={'runtime':'node','node':'24.21.0','bun':None,'executable':'/usr/local/bin/node'}
        t.validate_node(record)
        for change in [{'bun':'1.4.3'},{'node':'24.20.0'},{'runtime':'bun'},{'executable':'/usr/local/bin/bun'}]:
            with self.assertRaises(ValueError):t.validate_node({**record,**change})
    def test_npm_is_the_bundle_from_the_same_pinned_node_image(self):
        path='/usr/local/lib/node_modules/npm/bin/npm-cli.js'
        t.validate_npm('11.19.0','11.19.0',path)
        for actual,bundled,executable in [('success','11.19.0',path),('11.18.0','11.19.0',path),('11.19.0','11.19.0','/usr/local/bin/bun')]:
            with self.assertRaises(ValueError):t.validate_npm(actual,bundled,executable)

    def test_compose_client_versions_are_exact_not_success_placeholders(self):
        t.validate_compose('Docker version 29.9.2, build verified','v5.6.1')
        for docker,compose in [('Docker version 29.8.0, build old','v5.6.1'),('Docker version 29.9.2, build verified',''),('success','success')]:
            with self.assertRaises(ValueError):t.validate_compose(docker,compose)

if __name__=='__main__':unittest.main()
