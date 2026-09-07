#!/usr/bin/env python3
"""Public functions that nothing but their own tests calls.

# The pattern this reports

C14_TODO §10 lists six instances from one session of the same defect: code that
exists, compiles, is usually tested, and nothing calls it. `fonts::install`
across eleven apps, `d2x-job::preflight` after being ported back, `net::Blender`
sitting beside a `write_output_rgba8` that still hard-cropped,
`camera::viewfinder_height` outliving its call site. Only one of the six was
caught by a tool, and only because it was a whole FILE — `orphan_check.py`
cannot see a function.

`aligned_width` in snapclone's camera_policy is the shape exactly: a public
method, three assertions pinning it, and no production caller. The tests pass
and prove nothing about the app.

# A report, not a gate

This EXITS 0 always. The count is high — around 10% of public functions in a
large app crate — and a large fraction of those are legitimately fine: an app's
internal helper can be `pub` for module reasons and be reached through a trait,
a macro, or a `pub use` this scan does not follow.

Making it a gate would mean a hundred-entry allowlist that nobody maintains,
which is how `lock_check.py`'s first run (26 hits, three false classes) would
have gone if it had been shipped uncalibrated. Ranked output that a person
reads once after a merge is worth more than a red check nobody looks at.

# What it cannot see

Trait implementations, macro-generated calls, re-exports followed through
`pub use`, and anything called from a non-Rust build step. Treat every hit as a
question, not a verdict.

Usage:  python3 maintenance/tools/unwired_check.py [crate-dir ...]
        with no arguments, every crate under apps/ and system/
"""
import os, re, sys, subprocess
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

def strip_test_mods(text):
    """Remove every `#[cfg(test)] mod ... { ... }` by brace matching.

    Splitting on the FIRST `#[cfg(test)]` assumes test modules come last. They
    do not: d2x-job's job.rs has one at line 207 and public functions at 309,
    so the naive split hid a definition AND its two callers and reported it
    unwired.
    """
    out, i = [], 0
    while True:
        m = re.search(r'#\[cfg\(test\)\]', text[i:])
        if not m:
            out.append(text[i:]); break
        start = i + m.start()
        out.append(text[i:start])
        brace = text.find("{", start)
        if brace < 0:
            break
        depth, j = 0, brace
        while j < len(text):
            if text[j] == "{": depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0: break
            j += 1
        i = j + 1
    return "".join(out)

def leaf_crates():
    """Crates with code of their own, under apps/ and system/.

    Workspace roots are skipped: `apps/bscale` has no `src/` and all its code
    is in `crates/*`, so scanning the root finds nothing and reports a clean
    bill for an app it never looked at.

    shared/ is skipped entirely — those crates exist to be called from
    elsewhere, so "no caller in this crate" is their normal condition and every
    one would be a false hit.
    """
    out = []
    for base in ("apps", "system", "games"):
        for dp, dn, fn in os.walk(os.path.join(ROOT, base)):
            dn[:] = [d for d in dn if d not in {"target", ".git", "node_modules"}]
            if "Cargo.toml" in fn and os.path.isdir(os.path.join(dp, "src")):
                out.append(os.path.relpath(dp, ROOT))
    return sorted(out)


def scan(crate):
    src = os.path.join(ROOT, crate, "src")
    defs = {}
    for dp, dn, fn in os.walk(src):
        for f in fn:
            if not f.endswith(".rs"): continue
            p = os.path.join(dp, f)
            for m in re.finditer(r'^\s*pub (?:async )?fn (\w+)', open(p, errors="replace").read(), re.M):
                defs.setdefault(m.group(1), p)

    SKIP = {"default","fmt","clone","drop","len","is_empty","next","eq","hash","cmp","from","into"}
    out = []
    # ONE `git grep` for the whole crate, not one per function. The per-function
    # form spawns a process per name and takes minutes on a large crate, which
    # is the difference between a tool someone runs and one they do not.
    names = [n for n in defs if not n.startswith("new") and n not in SKIP]
    if not names:
        return len(defs), []
    pat = r"\b(" + "|".join(re.escape(n) for n in names) + r")\b"
    r = subprocess.run(["git", "grep", "-h", "-o", "-E", pat, "--", "*.rs"],
                       cwd=ROOT, capture_output=True, text=True)
    seen_elsewhere = {}
    r2 = subprocess.run(["git", "grep", "-o", "-E", pat, "--", "*.rs"],
                        cwd=ROOT, capture_output=True, text=True)
    for line in r2.stdout.splitlines():
        f, _, name = line.partition(":")
        name = name.rpartition(":")[2] or name
        seen_elsewhere.setdefault(name, set()).add(os.path.join(ROOT, f))

    for name in names:
        p = defs[name]
        files = seen_elsewhere.get(name, set())
        if any(os.path.realpath(f) != os.path.realpath(p) for f in files): continue
        code = strip_test_mods(open(p, errors="replace").read())
        if len(re.findall(r'\b'+re.escape(name)+r'\b', code)) <= 1:
            out.append((name, os.path.relpath(p, ROOT)))
    return len(defs), sorted(out)


def main():
    crates = sys.argv[1:] or leaf_crates()
    if not crates:
        print("no crates found — is the root right?")
        return 2
    total_defs = total_hits = 0
    rows = []
    for c in crates:
        if not os.path.isdir(os.path.join(ROOT, c, "src")):
            continue
        n, hits = scan(c)
        total_defs += n
        total_hits += len(hits)
        if hits:
            rows.append((len(hits), c, hits))
    rows.sort(reverse=True)
    for count, c, hits in rows:
        print(f"{c}: {count} public fns called only by their own tests")
        for name, f in hits[:8]:
            print(f"    {name}  ({f})")
        if len(hits) > 8:
            print(f"    … and {len(hits) - 8} more")
    print(f"\nscanned {len(crates)} crates, {total_defs} public fns, {total_hits} with no production caller")
    print("Every hit is a QUESTION: trait impls, macros and `pub use` re-exports")
    print("are invisible here. See the module note.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
