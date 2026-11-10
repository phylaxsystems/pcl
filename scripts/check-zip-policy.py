"""Reject vulnerable ZIP copies across the entire lockfile, including inactive targets.

Run with `uv run --python 3.12 python scripts/check-zip-policy.py`.
This deliberately does not depend on RustSec coverage or deny.toml ignores.
9.0.5 also fixes Zip Cloaking (GHSA-m665-v7pm-7grj); the three
2026-11-10 alerts (GHSA-8hxf-qr8p-62x4, GHSA-hjqx-j4rw-g7xv,
GHSA-9hj8-73fh-9963) require at least 9.0.0.
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
        if not match or tuple(map(int, match.groups())) < (9, 0, 5):
            errors.append(f"zip {version}: require a stable release >=9.0.5")
    return errors


if __name__ == "__main__":
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("Cargo.lock")
    with path.open("rb") as source:
        errors = violations(tomllib.load(source))
    if errors:
        sys.exit("ZIP dependency policy failed:\n" + "\n".join(errors))
    print("ZIP dependency policy passed")
