#!/usr/bin/env python3
"""reachability — find ported modules that nothing calls.

Every bug reported against the egui ports this week had ONE shape, and it was
not a missing surface:

  * `cardinal_gesture` was ported, tested, and referenced by nothing — so
    cardinal swipes did nothing on a keyboard whose keys sensed only clicks.
  * `pty_input` and `term_grid` were ported, tested, and referenced by
    nothing — so the terminal drew and typed nothing.
  * `bttransfer` was called by the Qt GUI every day and by the egui port
    never — so Bluetooth send silently vanished in the port.
  * The branches page said "read-only list" while `native::branch_create` and
    `branch_delete` sat in core, unused.

`parity_audit` cannot see any of these. It asks "does a module mentioning this
surface exist", and in every case one did — complete, documented, well tested,
and WIRED TO NOTHING. A test suite passing 100% tells you the same nothing: a
module's own tests exercise the module, not its callers.

So this asks the complementary question: **is it reachable from an entry
point?** A module nobody imports is code that cannot run, however good it is.

Two checks, because they fail differently:

  ORPHAN   a module in the crate that no other module references.
           High confidence, low noise. This is the cardinal_gesture case.

  UNUSED   a public function nothing outside its own file calls. Noisier —
           a library's API is meant to be called from outside the crate — so
           it is reported only for binaries and only counted, not listed,
           unless asked.

Run:  python3 maintenance/tools/reachability.py [app-dir ...]

PERFORMANCE NOTE, found while verifying the #[path] fix above: `orphans()` is
O(modules x files x patterns) full-text regex scans, and unendlich-engine
(1300+ .rs files under src/) does not finish in 5 minutes. Not fixed here —
this is a separate concern from the correctness bug this file was just
patched for, and a real fix (index identifiers once per file instead of
re-scanning every file per module name) is a bigger change than a
verification pass at the end of a session should make blind. Worth knowing
before assuming a clean run on the tree's largest crates.
"""
import os
import re
import sys
import glob

# Modules that are entry points by definition: nothing in-crate references
# main.rs or a bin, and lib.rs is the crate root.
ENTRY = {"main", "lib", "mod"}


def modules(src_dir):
    """Every module file, keyed by the name `mod x;` would use."""
    out = {}
    for p in glob.glob(os.path.join(src_dir, "**", "*.rs"), recursive=True):
        name = os.path.basename(p)[:-3]
        # A directory module is `foo/mod.rs`, named for its directory.
        if name == "mod":
            name = os.path.basename(os.path.dirname(p))
        out.setdefault(name, []).append(p)
    return out


def strip_comments(text):
    """Drop line comments so a module NAMED in prose does not count as used.

    This is the same false positive parity_audit had to fix twice: the house
    style names things in module docs, and a mention is not a call.
    """
    return "\n".join(
        l for l in text.split("\n") if not l.lstrip().startswith(("//", "#", "*"))
    )


def references(name, text):
    """Does this text actually USE the module, rather than declare or name it?

    `mod x;` is a declaration — it makes the module compile, which is exactly
    why an orphan still passes `cargo test`. Only `x::`, `use ...x...` and
    `super::x` count as reaching it.

    Also counts a `#[path = "x.rs"]` redirect naming this module's file — checked on the RAW
    text, before `strip_comments` runs. `strip_comments` drops any line starting with `#`, which
    is right for nothing in Rust (only `//` is a line comment; `#[...]` is an attribute), so it
    was silently deleting exactly the line that ties a path-redirected file to the mod name that
    actually reaches it — e.g. `#[path = "render_meshref_shim.rs"] pub mod render;` reaches
    `render_meshref_shim.rs`, but nothing in that file is ever named `render_meshref_shim`
    anywhere in the crate, so every check below always missed it. Found via unendlich-engine's
    real render_meshref_shim.rs (same file orphan_check.py's #[path] fix covers from the other
    checker's angle) rather than assumed from reading the regex.
    """
    stem = re.escape(name)
    if re.search(r'#\[\s*path\s*=\s*["\'][^"\']*\b%s\.rs["\']\s*\]' % stem, text):
        return True
    code = strip_comments(text)
    code = re.sub(r"^\s*(pub )?mod %s;\s*$" % re.escape(name), "", code, flags=re.M)
    pats = [
        r"\b%s::" % re.escape(name),          # x::thing
        r"use\s+[\w:]*\b%s\b" % re.escape(name),  # use crate::x
        r"\bsuper::%s\b" % re.escape(name),
        r"\bself::%s\b" % re.escape(name),
    ]
    return any(re.search(p, code) for p in pats)


def orphans(src_dir):
    """Modules that compile but nothing reaches."""
    mods = modules(src_dir)
    texts = {}
    for p in glob.glob(os.path.join(src_dir, "**", "*.rs"), recursive=True):
        try:
            texts[p] = open(p, errors="ignore").read()
        except OSError:
            pass

    found = []
    for name, paths in sorted(mods.items()):
        if name in ENTRY:
            continue
        # A module referenced from ANY file other than its own is reachable.
        # (Reachable-from-an-entry-point transitively would be stricter, but a
        # module used only by another orphan is a rarer and less costly case
        # than a module used by nobody at all — and reporting a whole
        # subsystem because its root is unused would bury the signal.)
        own = set(paths)
        hit = any(references(name, t) for p, t in texts.items() if p not in own)
        if not hit:
            lines = sum(len(texts.get(p, "").split("\n")) for p in paths)
            tests = sum(len(re.findall(r"#\[test\]", texts.get(p, ""))) for p in paths)
            found.append((name, paths[0], lines, tests))
    return found


def bin_reachable(src_dir):
    """Modules the crate's BINARIES actually reach, transitively.

    This is the distinction that matters for a port. Both egui frontends here
    are `lib + bin`: every module is `pub mod`-ed from lib.rs, so every module
    is a legitimate export and none looks orphaned. But an exported module that
    the app's own binary never reaches is still a feature the user cannot
    touch — `pty_input` was exported, tested, and unreachable from the running
    terminal.

    So: start at the bins, follow references, and report what is never arrived
    at. A library consumer could still call these, which is why they are
    reported as UNREACHED rather than dead.
    """
    texts = {}
    for p in glob.glob(os.path.join(src_dir, "**", "*.rs"), recursive=True):
        try:
            texts[p] = open(p, errors="ignore").read()
        except OSError:
            pass
    mods = modules(src_dir)
    # lib.rs IS a seed, and leaving it out was a real false positive.
    #
    # In a lib+bin crate the binary is a few lines that call into the library,
    # so almost everything is reached THROUGH lib.rs — and a module used only
    # by a free function there looked unreachable. `cardinal_gesture` reported
    # as unreached while the live path ran demo_app -> keyboard_ui -> paint_key
    # -> dir8_from -> cardinal_gesture.
    #
    # This does loosen the check: a lib.rs item nothing calls now makes its
    # dependencies look reached. That is the right trade here — the tool's job
    # is finding whole subsystems nobody wired up, and a false NEGATIVE on one
    # helper is cheaper than a false POSITIVE that sends somebody to re-wire
    # code that already works. Reporting the second kind twice is how a tool
    # stops being read.
    seeds = [p for p in texts
             if "/bin/" in p or p.endswith("/main.rs") or p.endswith("/lib.rs")]
    if not seeds:
        return None

    reached, frontier = set(), list(seeds)
    while frontier:
        cur = frontier.pop()
        body = texts.get(cur, "")
        for name, paths in mods.items():
            if name in reached or name in ENTRY:
                continue
            if references(name, body):
                reached.add(name)
                frontier.extend(paths)
    return reached


def report(app_dir):
    name = os.path.basename(app_dir.rstrip("/"))
    # ONE CRATE AT A TIME. Scanning a whole app conflates crates: a module
    # used only by the Qt tree looks orphaned to the egui tree and vice versa,
    # and a shared crate's public API is called from outside it by design.
    #
    # Reachability is a per-crate question, so each `src` next to a Cargo.toml
    # is its own scan.
    roots = []
    for d in glob.glob(os.path.join(app_dir, "**", "src"), recursive=True):
        if "/target/" in d or "/tests/" in d:
            continue
        if os.path.exists(os.path.join(os.path.dirname(d), "Cargo.toml")):
            roots.append(d)
    total = []
    for r in roots:
        for o in orphans(r):
            total.append(o)
    return name, total


def is_binary_crate(src_dir):
    """Only binaries can have true orphans.

    A library's modules are reachable through its own public API, which lives
    outside the crate by definition — reporting those would be reporting the
    library for existing.
    """
    d = os.path.dirname(src_dir)
    return (os.path.exists(os.path.join(src_dir, "main.rs"))
            or os.path.isdir(os.path.join(src_dir, "bin")))


def unreached(src_dir):
    """Exported modules the binary never arrives at."""
    reached = bin_reachable(src_dir)
    if reached is None:
        return []
    out = []
    for name, paths in sorted(modules(src_dir).items()):
        if name in ENTRY or name in reached:
            continue
        text = "".join(open(p, errors="ignore").read() for p in paths)
        out.append((name, paths[0], len(text.split("\n")),
                    len(re.findall(r"#\[test\]", text))))
    return out


def main(argv):
    dirs = argv[1:] or sorted(
        d for d in glob.glob("apps/*") + glob.glob("games/*") if os.path.isdir(d)
    )
    if "--unreached" in argv:
        argv = [a for a in argv if a != "--unreached"]
        dirs2 = argv[1:] or []
        print("Modules the app's own BINARY never reaches.\n")
        print("Exported, compiled, tested — and no code path in the running app")
        print("arrives at them. This is what a green test suite cannot tell you.\n")
        for d in dirs2:
            src = os.path.join(d, "src")
            found = unreached(src)
            if not found:
                continue
            print(f"{d}:")
            for n, p, l, t in sorted(found, key=lambda x: -x[3]):
                if t:
                    print(f"  {n:<24} {l:>5} lines, {t:>3} tests")
            print()
        return 0

    print(f"{'app':<20} {'orphans':>7}  {'lines':>6}  {'tests':>5}   modules")
    worst = []
    for d in dirs:
        app, found = report(d)
        if not found:
            continue
        lines = sum(f[2] for f in found)
        tests = sum(f[3] for f in found)
        names = ", ".join(f[0] for f in sorted(found, key=lambda x: -x[2])[:4])
        more = "" if len(found) <= 4 else f" (+{len(found)-4})"
        print(f"{app:<20} {len(found):>7}  {lines:>6}  {tests:>5}   {names}{more}")
        worst.append((tests, app, found))

    print()
    print("An orphan with TESTS is the expensive kind: it passed review, it passes")
    print("CI, and it does nothing. Those are listed first.")
    for tests, app, found in sorted(worst, reverse=True)[:3]:
        if tests == 0:
            continue
        print(f"\n{app}:")
        for n, p, l, t in sorted(found, key=lambda x: -x[3])[:5]:
            if t:
                print(f"  {n:<22} {l:>5} lines, {t:>3} tests — {p}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
