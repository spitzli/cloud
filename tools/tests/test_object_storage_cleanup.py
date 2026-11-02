import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("object_storage", Path(__file__).with_name("object-storage.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class CleanupFailures(unittest.TestCase):
    def test_unattached_policy_failure_does_not_skip_users_or_buckets(self):
        commands = []
        def mc(*args):
            commands.append(args)
            if args[:3] == ("admin", "policy", "remove"):
                raise RuntimeError("provider fixture failure")
            return ""
        owned = [("bucket", "b1"), ("bucket", "b2"), ("user", "u"), ("policy", "p")]
        with self.assertRaisesRegex(RuntimeError, "remove-policy"):
            fixture.cleanup_owned(mc, owned)
        self.assertNotIn(("admin", "policy", "detach", "fixture", "p", "--user", "u"), commands)
        self.assertIn(("admin", "user", "remove", "fixture", "u"), commands)
        for name in ["b1", "b2"]:
            self.assertIn(("rm", "--recursive", "--force", f"fixture/{name}"), commands)
            self.assertIn(("rb", f"fixture/{name}"), commands)

    def test_failed_detach_still_removes_user_then_policy_and_all_buckets(self):
        commands = []
        def mc(*args):
            commands.append(args)
            if args[:3] == ("admin", "policy", "detach"):
                raise RuntimeError("provider fixture failure")
            return ""
        owned = [("bucket", "b"), ("user", "u"), ("policy", "p"), ("attachment", ("p", "u"))]
        with self.assertRaisesRegex(RuntimeError, "detach-policy"):
            fixture.cleanup_owned(mc, owned)
        user = commands.index(("admin", "user", "remove", "fixture", "u"))
        policy = commands.index(("admin", "policy", "remove", "fixture", "p"))
        self.assertLess(user, policy)
        self.assertIn(("rb", "fixture/b"), commands)

    def test_absence_is_checked_and_foreign_resources_are_never_removed(self):
        commands = []
        def mc(*args):
            commands.append(args)
            if args == ("ls", "--json", "fixture"):
                return json.dumps({"key": "foreign/"})
            return ""
        fixture.cleanup_owned(mc, [("bucket", "owned")])
        self.assertIn(("ls", "--json", "fixture"), commands)
        self.assertFalse(any("fixture/foreign" in args for args in commands))


if __name__ == "__main__":
    unittest.main()
