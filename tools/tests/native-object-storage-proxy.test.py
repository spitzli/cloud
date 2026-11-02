import importlib.util
import pathlib
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location("proxy", pathlib.Path(__file__).with_name("native-object-storage-proxy.py"))
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)


class ProxyPolicy(unittest.TestCase):
    def test_fixed_private_tls_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            certificate = pathlib.Path(directory, "cert")
            key = pathlib.Path(directory, "key")
            for path in [certificate, key]:
                path.touch(mode=0o600)
            config = {"listen": "172.30.0.1", "port": 19443, "domain": "abcdef123456.native-object-storage.test", "certificate": str(certificate), "key": str(key), "deadline": time.time() + 3600, "upstreamIP": "172.25.0.2"}
            self.assertEqual(proxy.validate(config), config)
            for changes in [{"listen": "8.8.8.8"}, {"listen": "127.0.0.1"}, {"upstreamIP":"8.8.8.8"}, {"upstreamIP":"127.0.0.1"}, {"domain": "example.com"}, {"domain": "abcdef123456.native-object-storage.test.evil"}, {"deadline": time.time() - 1}, {"deadline": time.time() + 5 * 3600}, {"upstream": "http://foreign.test"}, {"skipVerify": True}]:
                with self.assertRaises((AssertionError, ValueError)):
                    proxy.validate({**config, **changes})
            key.chmod(0o644)
            with self.assertRaises(AssertionError):
                proxy.validate(config)


if __name__ == "__main__":
    unittest.main()
