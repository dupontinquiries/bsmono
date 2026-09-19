#!/usr/bin/env python3
"""mount_closure_check.py — will this `#[path]` mount actually compile?

A module mounted with `#[path = "../../src/foo.rs"]` is compiled as part of the
HOST crate, so every `crate::bar` inside it must resolve against the host's
module tree, not the tree it was written for. When it does not, the failure is
a wall of E0433s at build time and nothing before that.

This has cost the fleet twice:

  * voiceterm's `moment` was mounted on a scan that reported zero dependencies.
    It calls `crate::audio_profile::noise_floor()` and
    `crate::scroll_signals::is_reading_paced()` as INLINE paths, so a scan for
    `use crate::` lines saw nothing. Real closure: five modules, one of them
    reached only through the `clog!` macro expanding to `$crate::log::emit`.
  * `wildkit` was extracted with the same scan, before it was corrected, and
    shipped two inline `crate::util::mathx::*` calls that broke every build
    depending on it.

Both were the same mistake: looking for `use crate::` when the reference was
inline. This looks for `crate::` anywhere, and for the macros that expand to it.

## What it reports

For each `#[path]` mount, the first path segment of every `crate::X` reference
inside the mounted file, minus the modules the host declares. Anything left is
a name the host does not have and the mount cannot resolve.

False positives are possible and cheap: a `crate::X` inside a string literal,
or a host that declares `X` somewhere this does not parse. False NEGATIVES are
the expensive direction and this errs the other way — it counts a `clog!` as a
dependency on `log`, because that is what it expands to.

## Use

    mount_closure_check.py <repo-root>

Exit 1 if any mount has an unresolved reference.
"""
import os
import re
import sys

MOUNT = re.compile(
    r'#\[path\s*=\s*"([^"]+)"\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?mod\s+(\w+)\s*;'
)
# `crate::foo!(...)` is a macro invocation, not a module path — a
# `#[macro_export]` macro lives at the crate root and needs no `mod foo`. The
# negative lookahead for `!` is why: without it every `crate::clog!` call read
# as a dependency on a module named `clog`, and the tool reported 30 mounts
# that compile today.
CRATE_REF = re.compile(r"\bcrate::([a-z_][a-z0-9_]*)\b(?!\s*!)")
# Attributes may precede `mod` ON THE SAME LINE -- logscope writes
# `#[cfg(feature = "gui")] pub mod analysis;` throughout, and anchoring to
# line-start-then-`pub` missed every one, reporting four mounts as broken in a
# crate that builds.
MOD_DECL = re.compile(
    r"^\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*[;{]",
    re.M,
)
# `pub use nifty as niftyplot;` makes `crate::niftyplot` resolve just as a
# `mod` would. Counting only `mod` reported a real alias as a missing module.
# `pub use x as y;` AND `pub use other_crate::thing;` both put a name at the
# crate root that `crate::name` resolves to. logscope re-exports whole modules
# from sibling crates this way -- `pub use scene_kit::spatial;` -- and matching
# only the `as` form reported four mounts in a crate that builds.
USE_ALIAS = re.compile(
    r"^\s*(?:#\[[^\]]*\]\s*)*pub\s+use\s+(?:[\w:]+\s+as\s+([a-z_][a-z0-9_]*)"
    r"|(?:[\w]+::)*([a-z_][a-z0-9_]*))\s*;",
    re.M,
)

# Macros that expand to a `$crate::` path, so using one is a dependency on the
# module behind it even though no `crate::` appears at the call site.
MACRO_DEPS = {"clog!": "log"}


def crate_root_of(path: str) -> str:
    """The crate root governing `path`: the nearest `src/lib.rs` or `src/main.rs`.

    A mount inside `src/foo.rs` still resolves `crate::bar` against the CRATE's
    module tree, not against `foo.rs`. Reading declarations from the file
    holding the mount was this tool's own first bug: it reported
    `missing_pkg.rs` as unable to see `log`, when `log` is declared in the
    lib.rs two directories up and the crate builds.
    """
    d = os.path.dirname(os.path.abspath(path))
    while True:
        for cand in ("lib.rs", "main.rs"):
            p = os.path.join(d, cand)
            if os.path.isfile(p):
                return p
        parent = os.path.dirname(d)
        if parent == d:
            return path
        d = parent


def host_modules(root_file: str) -> set:
    """Every module the host crate declares, transitively.

    Follows `mod x;` into `x.rs` / `x/mod.rs` so a module declared one level
    down still counts -- `crate::x` is reachable from anywhere in the crate.
    """
    seen_files, mods, queue = set(), set(), [crate_root_of(root_file)]
    while queue:
        f = queue.pop()
        if f in seen_files or not os.path.isfile(f):
            continue
        seen_files.add(f)
        try:
            with open(f, errors="replace") as fh:
                text = fh.read()
        except OSError:
            continue
        here = (set(MOD_DECL.findall(text))
                | {n for pair in USE_ALIAS.findall(text) for n in pair if n}
                | {m.group(2) for m in MOUNT.finditer(text)})
        mods |= here
        base = os.path.dirname(f)
        for name in here:
            queue.append(os.path.join(base, name + ".rs"))
            queue.append(os.path.join(base, name, "mod.rs"))
    return mods


def strip_line_comments(body: str) -> str:
    """Drop `//` comments so prose mentioning `crate::x` is not a finding."""
    out = []
    for ln in body.split("\n"):
        if ln.strip().startswith("//"):
            out.append("")
        else:
            out.append(ln.split("//")[0])
    return "\n".join(out)


def main(argv):
    if len(argv) != 2:
        print(__doc__)
        return 2
    repo = argv[1]
    problems = []
    checked = 0
    for base, dirs, files in os.walk(repo):
        dirs[:] = [d for d in dirs if d not in (".git", "target", "node_modules")]
        for f in files:
            if not f.endswith(".rs"):
                continue
            root_file = os.path.join(base, f)
            try:
                with open(root_file, errors="replace") as fh:
                    text = fh.read()
            except OSError:
                continue
            mounts = list(MOUNT.finditer(text))
            if not mounts:
                continue
            declared = host_modules(root_file)
            for m in mounts:
                rel, name = m.group(1), m.group(2)
                target = os.path.normpath(os.path.join(base, rel))
                if not os.path.isfile(target):
                    problems.append((root_file, name, rel, ["<file not found>"]))
                    continue
                checked += 1
                with open(target, errors="replace") as fh:
                    code = strip_line_comments(fh.read())
                needs = set(CRATE_REF.findall(code))
                for mac, dep in MACRO_DEPS.items():
                    if mac in code:
                        needs.add(dep)
                missing = sorted(needs - declared - {name})
                if missing:
                    problems.append((root_file, name, rel, missing))

    if not problems:
        print(f"{checked} #[path] mounts checked — every crate:: reference resolves")
        return 0
    print(f"{len(problems)} mount(s) reference names the host crate does not declare:\n")
    for host, name, rel, missing in problems:
        print(f"  {host}")
        print(f"    mod {name}  ({rel})")
        print(f"    needs: {', '.join(missing)}")
    print("\nA #[path] module compiles as part of the HOST crate, so its `crate::`")
    print("paths resolve against the host. Declare the missing modules, mount them")
    print("too, or do not mount this one.")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
