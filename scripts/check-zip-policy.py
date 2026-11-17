"""Reject vulnerable zip copies in the lockfile and in every manifest.

Run with `uv run --python 3.12 python scripts/check-zip-policy.py`.
The floor is 9.1.3. GHSA-66qv-gmq4-qcqj, uncontrolled recursion in the
Shrink decoder, was published on 2026-11-10. Releases through 9.1.2 are
affected, and 9.1.3 is the fixed release.

Cargo.lock records one resolution. A caret requirement such as 9.0.5, a
`[patch]` entry, or `zip = { path = "..." }` can still select an older
release when Cargo resolves without that lockfile.
"""

import re
import sys
import tomllib
from pathlib import Path

FLOOR = (9, 1, 3)
_DEP_KEYS = {"dependencies", "dev-dependencies", "build-dependencies"}
_SKIP_DIRS = {"target", ".git"}
_STABLE = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:\+[0-9A-Za-z.-]+)?")
_OPS = (
    (">=", "gte"),
    ("<=", "lte"),
    (">", "gt"),
    ("<", "lt"),
    ("=", "eq"),
    ("~", "tilde"),
    ("^", "caret"),
)


def violations(lockfile):
    errors = []
    for package in lockfile["package"]:
        if package["name"] != "zip":
            continue
        found = _release_violation(package["version"])
        if found:
            errors.append(found)
    return errors


def requirement_violation(requirement):
    if not isinstance(requirement, str) or not requirement.strip():
        return f"zip requirement {requirement!r} is not a Cargo version requirement"
    parsed = _parse_req(requirement.strip())
    if parsed is None:
        return f"zip requirement {requirement!r} is not a Cargo version requirement"
    if parsed == "star":
        return f"zip requirement {requirement!r} allows a release below 9.1.3"
    if any(item.pre for item in parsed):
        return f"zip requirement {requirement!r} includes a prerelease"
    if _exists(parsed, below=True):
        return f"zip requirement {requirement!r} allows a release below 9.1.3"
    if not _exists(parsed, below=False):
        return f"zip requirement {requirement!r} matches no stable release >=9.1.3"
    return None


def scan(root):
    root = Path(root)
    errors = []
    lockfiles = sorted(_tracked(root, "Cargo.lock"))
    if not lockfiles:
        errors.append(f"{_display(root / 'Cargo.lock', root)}: missing lockfile")
    for lockfile in lockfiles:
        try:
            with lockfile.open("rb") as source:
                found = violations(tomllib.load(source))
        except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
            errors.append(f"{_display(lockfile, root)}: {error}")
            continue
        origin = _display(lockfile, root)
        errors.extend(f"{origin}: {item}" for item in found)
    for manifest in sorted(_tracked(root, "Cargo.toml")):
        errors.extend(_manifest_violations(manifest, root))
    return errors


def _release_violation(version):
    match = _STABLE.fullmatch(version) if isinstance(version, str) else None
    if not match or tuple(map(int, match.groups())) < FLOOR:
        return f"zip {version}: require a stable release >=9.1.3"
    return None


def _manifest_violations(manifest, root):
    origin = _display(manifest, root)
    try:
        with manifest.open("rb") as source:
            data = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [f"{origin}: {error}"]
    errors = []
    package = data.get("package")
    if isinstance(package, dict) and package.get("name") == "zip":
        found = _release_violation(package.get("version"))
        if found:
            errors.append(f"{origin}: {found}")
    for table in _dependency_tables(data):
        errors.extend(_spec_violations(table, origin, manifest))
    return errors


def _dependency_tables(node, trail=()):
    if not isinstance(node, dict):
        return
    for key, value in node.items():
        if not isinstance(value, dict):
            continue
        parent = trail[-1] if trail else ""
        if key in _DEP_KEYS or parent == "patch":
            yield value
        yield from _dependency_tables(value, (*trail, key))


def _spec_violations(table, origin, manifest):
    errors = []
    for key, value in table.items():
        name = key
        if isinstance(value, dict) and isinstance(value.get("package"), str):
            name = value["package"]
        if name != "zip":
            continue
        errors.extend(_zip_spec(value, f"{origin}: ", manifest))
    return errors


def _zip_spec(value, prefix, manifest):
    if isinstance(value, str):
        found = requirement_violation(value)
        return [prefix + found] if found else []
    if not isinstance(value, dict):
        return [prefix + "zip dependency is not a version requirement"]
    errors = []
    if "version" in value:
        found = requirement_violation(value["version"])
        if found:
            errors.append(prefix + found)
    elif value.get("workspace") is True:
        pass
    elif "git" in value:
        errors.append(prefix + "zip git source has no version requirement >=9.1.3")
    elif "path" not in value:
        errors.append(prefix + "zip dependency has no version requirement >=9.1.3")
    if "path" in value:
        errors.extend(_path_violations(value["path"], prefix, manifest))
    return errors


def _path_violations(path_value, prefix, manifest):
    if not isinstance(path_value, str):
        return [prefix + "zip path is not a path"]
    target = manifest.parent / path_value / "Cargo.toml"
    if not target.is_file():
        return [prefix + f"zip path {path_value} has no Cargo.toml"]
    try:
        with target.open("rb") as source:
            data = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [prefix + f"zip path {path_value}: {error}"]
    package = data.get("package") if isinstance(data, dict) else None
    if not isinstance(package, dict) or package.get("name") != "zip":
        name = package.get("name") if isinstance(package, dict) else None
        return [prefix + f"zip path {path_value} is package {name}"]
    found = _release_violation(package.get("version"))
    if found:
        return [prefix + f"zip path {path_value} {found}"]
    return []


class _Cmp:
    def __init__(self, op, major, minor, patch, pre):
        self.op = op
        self.major = major
        self.minor = minor
        self.patch = patch
        self.pre = pre


def _parse_req(text):
    if text in {"*", "x", "X"}:
        return "star"
    comparators = []
    for part in text.split(","):
        parsed = _parse_comparator(part.strip())
        if parsed is None:
            return None
        comparators.append(parsed)
    return comparators


def _parse_comparator(text):
    op = "caret"
    explicit = False
    for symbol, name in _OPS:
        if text.startswith(symbol):
            op = name
            text = text[len(symbol) :].strip()
            explicit = True
            break
    if text in {"*", "x", "X"} or not text:
        return None
    pre = ""
    if "+" in text:
        text, build = text.split("+", 1)
        if not _identifier(build):
            return None
    if "-" in text:
        text, pre = text.split("-", 1)
        if not pre or not _identifier(pre):
            return None
    pieces = text.split(".")
    if len(pieces) > 3 or not _number(pieces[0]):
        return None
    major = int(pieces[0])
    minor = None
    patch = None
    wildcard = False
    if len(pieces) >= 2:
        if pieces[1] in {"*", "x", "X"}:
            if len(pieces) != 2:
                return None
            wildcard = True
        elif _number(pieces[1]):
            minor = int(pieces[1])
        else:
            return None
    if len(pieces) == 3:
        if pieces[2] in {"*", "x", "X"}:
            wildcard = True
        elif _number(pieces[2]):
            patch = int(pieces[2])
        else:
            return None
    if pre and patch is None:
        return None
    if wildcard and not explicit:
        op = "wildcard"
    return _Cmp(op, major, minor, patch, pre)


def _number(text):
    return bool(text) and text.isdigit() and not (len(text) > 1 and text.startswith("0"))


def _identifier(text):
    segments = text.split(".")
    if not segments or any(
        not segment or not re.fullmatch(r"[0-9A-Za-z-]+", segment) for segment in segments
    ):
        return False
    for segment in segments:
        if segment.isdigit() and len(segment) > 1 and segment.startswith("0"):
            return False
    return True


def _exists(comparators, *, below):
    if below:
        majors = range(FLOOR[0] + 1)
    else:
        majors = {FLOOR[0], FLOOR[0] + 1}
        for item in comparators:
            if item.major >= FLOOR[0]:
                majors.add(item.major)
                majors.add(item.major + 1)
        majors = sorted(majors)
    minors = _seeds(item.minor for item in comparators)
    patches = _seeds(item.patch for item in comparators)
    for major in majors:
        for minor in minors:
            for patch in patches:
                version = (major, minor, patch)
                if below and version >= FLOOR:
                    continue
                if not below and version < FLOOR:
                    continue
                if all(_matches(item, major, minor, patch) for item in comparators):
                    return True
    return False


def _seeds(values):
    seeds = {0}
    for value in values:
        if value is None:
            continue
        seeds.add(value)
        seeds.add(value + 1)
    seeds.add(FLOOR[1])
    seeds.add(FLOOR[1] + 1)
    seeds.add(FLOOR[2])
    seeds.add(FLOOR[2] + 1)
    return sorted(seeds)


def _matches(item, major, minor, patch):
    if item.op in {"eq", "wildcard"}:
        return _exact(item, major, minor, patch)
    if item.op == "gt":
        return _greater(item, major, minor, patch)
    if item.op == "gte":
        return _exact(item, major, minor, patch) or _greater(item, major, minor, patch)
    if item.op == "lt":
        return _less(item, major, minor, patch)
    if item.op == "lte":
        return _exact(item, major, minor, patch) or _less(item, major, minor, patch)
    if item.op == "tilde":
        return _tilde(item, major, minor, patch)
    if item.op == "caret":
        return _caret(item, major, minor, patch)
    return False


def _exact(item, major, minor, patch):
    if major != item.major:
        return False
    if item.minor is not None and minor != item.minor:
        return False
    return item.patch is None or patch == item.patch


def _greater(item, major, minor, patch):
    if major != item.major:
        return major > item.major
    if item.minor is None or (minor == item.minor and item.patch is None):
        return False
    if minor != item.minor:
        return minor > item.minor
    if item.patch is None or patch == item.patch:
        return False
    return patch > item.patch


def _less(item, major, minor, patch):
    if major != item.major:
        return major < item.major
    if item.minor is None or (minor == item.minor and item.patch is None):
        return False
    if minor != item.minor:
        return minor < item.minor
    if item.patch is None or patch == item.patch:
        return False
    return patch < item.patch


def _tilde(item, major, minor, patch):
    if major != item.major:
        return False
    if item.minor is not None and minor != item.minor:
        return False
    return item.patch is None or patch >= item.patch


def _caret(item, major, minor, patch):
    if major != item.major:
        return False
    if item.minor is None:
        return True
    if item.patch is None:
        if item.major > 0:
            return minor >= item.minor
        return minor == item.minor
    if item.major > 0:
        if minor != item.minor:
            return minor > item.minor
        return patch >= item.patch
    if item.minor > 0:
        if minor != item.minor:
            return False
        return patch >= item.patch
    return minor == item.minor and patch == item.patch


def _tracked(root, name):
    for path in root.rglob(name):
        if not (_SKIP_DIRS & set(path.relative_to(root).parts)):
            yield path


def _display(path, root):
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


if __name__ == "__main__":
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else Path()
    if target.is_file():
        target = target.parent
    found = scan(target)
    if found:
        sys.exit("ZIP dependency policy failed:\n" + "\n".join(found))
    print("ZIP dependency policy passed")
