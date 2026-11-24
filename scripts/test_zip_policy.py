import importlib.util
import tempfile
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
            "9.1.3",
            "9.2.3",
            "9.3.0",
            "9.4.0",
            "9.4.1",
            "9.4.2-rc.1",
        ):
            with self.subTest(version=version):
                self.assertTrue(self.check(version))

    def test_accepts_fixed_versions_and_build_metadata(self):
        self.assertEqual(self.check("9.4.2", "9.8.2", "10.0.0", "9.4.2+build.1"), [])

    def test_fixed_copy_cannot_hide_a_vulnerable_duplicate(self):
        self.assertEqual(len(self.check("9.4.2", "0.6.6", "7.2.0")), 2)

    def test_allows_removing_zip_entirely(self):
        self.assertEqual(policy.violations({"package": [{"name": "other", "version": "1.0.0"}]}), [])

    def test_requirement_rejects_ranges_that_admit_a_release_below_the_floor(self):
        for requirement in (
            "9.0.5",
            "^9.0.5",
            "9.1",
            "9.1.0",
            "9.1.1",
            "9.1.2",
            "9.1.3",
            "^9.1.3",
            "=9.1.3",
            "~9.1.3",
            ">=9.1.3",
            ">9.1.2",
            ">=9.1.3, <10",
            "9.2.*",
            "9.3.0",
            "~9.1.2",
            ">=9.0.5",
            ">9.0.5",
            "*",
            "9.1.*",
            "9.4.*",
            "=9.1.2",
            "=9.4.1",
            "^9.4.1",
            "~9.4.1",
            "9.1.3-rc.1",
            "9.4.2-rc.1",
            ">=9.1.3, <9.1.3",
            ">=9.4.2, <9.4.2",
        ):
            with self.subTest(requirement=requirement):
                self.assertIsNotNone(policy.requirement_violation(requirement))

    def test_requirement_accepts_ranges_whose_only_matches_are_fixed_releases(self):
        for requirement in (
            "9.4.2",
            "^9.4.2",
            "=9.4.2",
            "~9.4.2",
            ">=9.4.2",
            ">9.4.1",
            ">=9.4.2, <10",
            "9.8.2",
            "9.5.*",
            ">9",
        ):
            with self.subTest(requirement=requirement):
                self.assertIsNone(policy.requirement_violation(requirement))

    def test_scan_rejects_patched_manifests_patch_entries_and_single_entry_paths(self):
        root = self.workspace(
            {
                "patches/soldeer-core/Cargo.toml": """
[dependencies.zip]
version = "9.0.5"
features = ["deflate"]
""",
                "patches/inline/Cargo.toml": """
[dependencies]
zip = "9.1.2"
""",
                "Cargo.toml": """
[patch.crates-io]
zip = { path = "old-zip" }
other = { git = "https://example.invalid/other.git", version = "9.0.5" }
""",
                "old-zip/Cargo.toml": """
[package]
name = "zip"
version = "0.6.6"
""",
            }
        )
        errors = policy.scan(root)
        self.assertTrue(any("patches/soldeer-core/Cargo.toml" in error and "9.0.5" in error for error in errors))
        self.assertTrue(any("patches/inline/Cargo.toml" in error and "9.1.2" in error for error in errors))
        self.assertTrue(any("old-zip" in error and "0.6.6" in error for error in errors))

    def test_scan_rejects_a_patch_version_and_an_unpinned_git_zip(self):
        root = self.workspace(
            {
                "Cargo.toml": """
[patch.crates-io]
zip = { git = "https://example.invalid/zip.git", rev = "abc" }
""",
            }
        )
        errors = policy.scan(root)
        self.assertTrue(errors)

        pinned = self.workspace(
            {
                "Cargo.toml": """
[patch.crates-io]
zip = { version = "9.0.5", path = "vendor/zip" }
""",
                "vendor/zip/Cargo.toml": """
[package]
name = "zip"
version = "9.4.2"
""",
            }
        )
        pinned_errors = policy.scan(pinned)
        self.assertTrue(any("9.0.5" in error for error in pinned_errors))

    def test_scan_accepts_a_fixed_manifest_and_a_fixed_path_patch(self):
        root = self.workspace(
            {
                "Cargo.toml": """
[patch.crates-io]
zip = { path = "vendor/zip" }
soldeer-core = { path = "patches/soldeer-core" }
candle-core = { path = "patches/soldeer-core", version = "0.10.2" }

[dependencies.zip]
version = "9.4.2"
default-features = false
""",
                "vendor/zip/Cargo.toml": """
[package]
name = "zip"
version = "9.4.2"
""",
                "patches/soldeer-core/Cargo.toml": """
[dependencies]
zip = { version = "9.4.2", default-features = false, features = ["deflate"] }
""",
                "member/Cargo.toml": """
[target.'cfg(unix)'.build-dependencies]
zip = { version = ">=9.4.2", package = "zip" }
""",
            }
        )
        self.assertEqual(policy.scan(root), [])

    def test_scan_prunes_build_and_git_directories(self):
        root = self.workspace(
            {
                "target/debug/Cargo.toml": """
[dependencies]
zip = "0.6.6"
""",
                "crates/pkg/target/Cargo.lock": """
[[package]]
name = "zip"
version = "0.6.6"
""",
                ".git/modules/Cargo.toml": """
[dependencies]
zip = "9.0.5"
""",
                "kept/Cargo.toml": """
[dependencies]
zip = "9.4.1"
""",
            }
        )
        errors = policy.scan(root)
        self.assertTrue(any("kept/Cargo.toml" in error and "9.4.1" in error for error in errors))
        self.assertFalse(any("target/" in error or ".git/" in error for error in errors))

    def test_this_workspace_admits_no_zip_below_the_floor(self):
        repo = Path(__file__).resolve().parents[1]
        self.assertEqual(policy.scan(repo), [])

    def workspace(self, files):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        (root / "Cargo.lock").write_text(
            '[[package]]\nname = "zip"\nversion = "9.4.2"\n', encoding="utf-8"
        )
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text.strip() + "\n", encoding="utf-8")
        return root


if __name__ == "__main__":
    unittest.main()
