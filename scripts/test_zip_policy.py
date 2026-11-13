import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "zip_policy", Path(__file__).with_name("check-zip-policy.py")
)
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ZipPolicyTest(unittest.TestCase):
    def check(self, *versions):
        return policy.violations(
            {"package": [{"name": "zip", "version": v} for v in versions]}
        )

    def test_rejects_every_affected_copy_and_prereleases(self):
        for version in (
            "0.6.6",
            "4.6.1",
            "7.2.0",
            "9.0.0",
            "9.0.4",
            "9.0.5",
            "9.0.6",
            "9.1.0",
            "9.1.1",
            "9.1.2",
            "9.3.0-rc.1",
        ):
            with self.subTest(version=version):
                self.assertTrue(self.check(version))

    def test_accepts_fixed_versions_and_build_metadata(self):
        self.assertEqual(self.check("9.1.3", "9.3.0", "10.0.0", "9.3.0+build.1"), [])

    def test_fixed_copy_cannot_hide_a_vulnerable_duplicate(self):
        self.assertEqual(len(self.check("9.3.0", "0.6.6", "7.2.0")), 2)

    def test_allows_removing_zip_entirely(self):
        self.assertEqual(policy.violations({"package": [{"name": "other", "version": "1.0.0"}]}), [])


if __name__ == "__main__":
    unittest.main()
