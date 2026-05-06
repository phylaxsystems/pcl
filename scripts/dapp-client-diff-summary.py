#!/usr/bin/env python3
"""Emit a markdown summary of public API surface changes in the generated client.

Compares the committed `client.rs` (HEAD) against the working-tree version
produced by `make regenerate`, extracts public fns / structs / enums via
regex, and reports added / removed / modified symbols. Output goes to stdout
and is expected to be captured into a GitHub Actions step output.

Best-effort regex, not a full parser: nested generics at the declaration
(e.g. `fn f<T: Map<K, V>>`), `pub const fn`, and `pub unsafe fn` are not
matched. Fine for progenitor output; check the Files tab if a symbol is missing.
"""

import re
import subprocess
import sys

CLIENT_PATH = "crates/dapp-api-client/src/generated/client.rs"

DOC_ATTR_RE = re.compile(r'#\s*\[\s*doc\s*=\s*"(?:[^"\\]|\\.)*"\s*\]')
LINE_DOC_RE = re.compile(r'//[/!][^\n]*')
FN_RE = re.compile(r'pub(?:\([^)]+\))?\s+(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)')
STRUCT_RE = re.compile(
    r'pub\s+struct\s+(\w+)\s*(?:<[^>]*>)?\s*(\{[^}]*\}|\([^)]*\)\s*;|;)'
)
ENUM_RE = re.compile(r'pub\s+enum\s+(\w+)\s*(?:<[^>]*>)?\s*(\{[^}]*\})')

WS_RE = re.compile(r"\s+")


def load_old():
    result = subprocess.run(
        ["git", "show", f"HEAD:{CLIENT_PATH}"],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout if result.returncode == 0 else None


def load_new():
    try:
        with open(CLIENT_PATH, "r", encoding="utf-8") as f:
            return f.read()
    except FileNotFoundError:
        return None


def strip_docs(src):
    src = DOC_ATTR_RE.sub("", src)
    src = LINE_DOC_RE.sub("", src)
    return src


def normalize(s):
    return WS_RE.sub(" ", s).strip()


def canon(s):
    return WS_RE.sub("", s)


def extract_items(src):
    src = strip_docs(src)
    fns = {name: normalize(params) for name, params in FN_RE.findall(src)}
    structs = {name: canon(body) for name, body in STRUCT_RE.findall(src)}
    enums = {name: canon(body) for name, body in ENUM_RE.findall(src)}
    return fns, structs, enums


def diff_keys(old, new):
    added = sorted(set(new) - set(old))
    removed = sorted(set(old) - set(new))
    modified = sorted(k for k in set(old) & set(new) if old[k] != new[k])
    return added, removed, modified


def render_section(title, items):
    if not items:
        return None
    header = f"**{title}** ({len(items)}):"
    body = "\n".join(f"- {item}" for item in items)
    return f"{header}\n{body}"


def main() -> int:
    old_src = load_old()
    new_src = load_new()

    if new_src is None:
        print("_Could not read new `client.rs`._")
        return 0
    if old_src is None:
        print("_No previous `client.rs` in HEAD — first committed version._")
        return 0

    old_fns, old_structs, old_enums = extract_items(old_src)
    new_fns, new_structs, new_enums = extract_items(new_src)

    added_fns, removed_fns, modified_fns = diff_keys(old_fns, new_fns)
    added_structs, removed_structs, modified_structs = diff_keys(old_structs, new_structs)
    added_enums, removed_enums, modified_enums = diff_keys(old_enums, new_enums)

    added = (
        [f"`{name}({new_fns[name]})` (method)" for name in added_fns]
        + [f"`{name}` (struct)" for name in added_structs]
        + [f"`{name}` (enum)" for name in added_enums]
    )
    removed = (
        [f"`{name}` (method)" for name in removed_fns]
        + [f"`{name}` (struct)" for name in removed_structs]
        + [f"`{name}` (enum)" for name in removed_enums]
    )
    modified = (
        [
            f"`{name}` (method): `({old_fns[name]})` → `({new_fns[name]})`"
            for name in modified_fns
        ]
        + [f"`{name}` (struct, fields changed)" for name in modified_structs]
        + [f"`{name}` (enum, variants changed)" for name in modified_enums]
    )

    sections = [
        render_section("Added", added),
        render_section("Removed", removed),
        render_section("Modified", modified),
    ]
    sections = [s for s in sections if s]

    if not sections:
        print("_No public API surface changes — only internal or documentation differences._")
    else:
        print("\n\n".join(sections))
    return 0


if __name__ == "__main__":
    sys.exit(main())
