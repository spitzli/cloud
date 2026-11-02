import importlib.util
from pathlib import Path
import unittest
spec=importlib.util.spec_from_file_location('ci_egress',Path(__file__).with_name('egress.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class Egress(unittest.TestCase):
    def test_only_ci_output_and_exact_dns_registry_exceptions(self):
        raw=module.rules()
        self.assertIn('table inet spitzli_plugin_ci',raw)
        self.assertIn('hook output',raw)
        self.assertNotIn('hook input',raw)
        self.assertNotIn('hook forward',raw)
        self.assertNotIn('flush ruleset',raw)
        self.assertEqual(raw.count('meta skuid 1001'),5)
        self.assertIn('127.0.0.53 udp dport 53',raw)
        self.assertIn('127.0.0.53 tcp dport 53',raw)
        self.assertIn('79.143.177.157 tcp dport 443',raw)
        self.assertLess(raw.index('tcp dport 443'),raw.index('IPv4 denied'))
        for address in ['10.0.0.0/8','169.254.0.0/16','127.0.0.0/8','fc00::/7','fe80::/10','::1/128','167.86.75.144','2a02:c207:3021:866::1']:
            self.assertIn(address,raw)

if __name__=='__main__':unittest.main()
