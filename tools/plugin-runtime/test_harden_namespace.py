import copy
import unittest
from harden_namespace import documents, policy_digest, verify


class NamespaceBoundaryTests(unittest.TestCase):
    def values(self):
        return ("wd-plugin-" + "a" * 24, "12345678-1234-1234-1234-123456789012", "b" * 64, 500, 268435456)

    def test_boundary_is_finite_and_cannot_allocate_persistent_or_public_resources(self):
        quota, limits, policy = documents(*self.values())
        self.assertEqual(quota["spec"]["hard"]["pods"], "1")
        for resource in ["persistentvolumeclaims", "requests.storage", "services.nodeports", "services.loadbalancers"]:
            self.assertEqual(quota["spec"]["hard"][resource], "0")
        self.assertEqual(limits["spec"]["limits"][0]["max"]["memory"], "256Mi")
        self.assertEqual(policy["spec"], {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]})

    def test_rejects_non_plugin_namespace_wrong_identity_and_unbounded_resources(self):
        for index, value in [(0, "default"), (1, "foreign"), (2, "latest"), (3, 0), (4, 0), (3, 64001)]:
            args = list(self.values()); args[index] = value
            with self.subTest(index=index, value=value), self.assertRaises(ValueError):
                documents(*args)

    def test_refuses_foreign_replaced_or_weakened_boundary(self):
        desired = documents(*self.values())[0]
        verify(copy.deepcopy(desired), desired)
        for mutation in [lambda x: x["metadata"]["ownerReferences"][0].update(uid="foreign"),
                         lambda x: x["spec"]["hard"].update(pods="100"),
                         lambda x: x["metadata"].update(deletionTimestamp="now")]:
            actual = copy.deepcopy(desired); mutation(actual)
            with self.assertRaises(ValueError):
                verify(actual, desired)

    def test_reviewed_policy_digest_detects_added_allow_all_egress(self):
        policy = documents(*self.values())[2]
        reviewed = policy_digest(policy)
        policy["spec"]["egress"] = [{}]
        self.assertNotEqual(policy_digest(policy), reviewed)

    def test_memory_quantities_match_kubernetes_canonical_units(self):
        for amount, canonical in [(1073741824, "1Gi"), (8389632, "8193Ki"), (1000000000, "1G"), (16000000, "16M")]:
            args = list(self.values()); args[4] = amount
            quota, limits, _ = documents(*args)
            self.assertEqual(quota["spec"]["hard"]["limits.memory"], canonical)
            self.assertEqual(limits["spec"]["limits"][0]["max"]["memory"], canonical)


if __name__ == "__main__":
    unittest.main()
