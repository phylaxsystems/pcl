"""Reject vulnerable ZIP copies across the entire lockfile, including inactive targets.

Run with `uv run --python 3.12 python scripts/check-zip-policy.py`.
This deliberately does not depend on RustSec coverage or deny.toml ignores.
The floor is 9.1.3. GHSA-66qv-gmq4-qcqj, uncontrolled recursion in the
Shrink decoder, was published on 2026-11-10. Releases through 9.1.2 are
affected, and 9.1.3 is the fixed release.
"""

import re
import sys
import tomllib
from pathlib import Path


def violations(lockfile):
    errors = []
    for package in lockfile["package"]:
        if package["name"] != "zip":
            continue
        version = package["version"]
        # Require a stable release; prereleases do not establish remediation.
        match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:\+[0-9A-Za-z.-]+)?", version)
        if not match or tuple(map(int, match.groups())) < (9, 1, 3):
            errors.append(f"zip {version}: require a stable release >=9.1.3")
    return errors


if __name__ == "__main__":
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("Cargo.lock")
    with path.open("rb") as source:
        errors = violations(tomllib.load(source))
    if errors:
        sys.exit("ZIP dependency policy failed:\n" + "\n".join(errors))
    print("ZIP dependency policy passed")
