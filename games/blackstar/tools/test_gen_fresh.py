#!/usr/bin/env python3
"""test_gen_fresh.py -- generated headers must match the source they claim to mirror.

`content_gen.h` says at the top that it mirrors `content/chassis.json`, and
`test_content_gen` checks it against the hand-written C table in `chassis.h`.
That is a useful check and it is not this one: it compares two GENERATED-AND-
HANDWRITTEN artifacts against each other, so if the JSON changes and nobody
re-runs the generator, the header and the C table stay in agreement -- both
stale -- and the suite passes while the stated source of truth has moved.

Its own header comment is honest about the gap: "making the JSON a *verified*
mirror, the first step toward single-source-of-truth". This is the step after.

The check is the only one that can catch it: re-run the generator into a
scratch directory and diff. Anything else is a consistency test, and a
consistency test on a generated file reads exactly like a freshness test
without being one.

  ./tools/test_gen_fresh.py [--root DIR]
"""
import difflib
import os
import shutil
import subprocess
import sys
import tempfile

root = "."
if "--root" in sys.argv:
    root = sys.argv[sys.argv.index("--root") + 1]
root = os.path.abspath(root)

# (generated header, generator argv, what it mirrors)
TARGETS = [
    (
        os.path.join("engine_src", "exengine", "content_gen.h"),
        [sys.executable, os.path.join("tools", "content_build.py"), "--emit-c", "--quiet"],
        os.path.join("content", "chassis.json"),
    ),
]

# Three outcomes, deliberately. A check that cannot run must say so rather
# than report a failure: `content_build.py` refuses to emit while any content
# error stands, and on a fresh checkout five part_group meshes are absent
# (see tools/sandbox_build.sh), so the generator exits non-zero before it
# writes anything. Calling that STALE would add a third permanently-red test
# to a suite that already has two, and a suite people learn to ignore catches
# nothing at all. Calling it fresh would be a lie. So: skipped, loudly, and
# non-zero only when a diff was actually seen.
bad = 0
skipped = 0
for header, argv, source in TARGETS:
    hp = os.path.join(root, header)
    if not os.path.exists(hp):
        print(f"MISSING: {header} does not exist; run its generator")
        bad += 1
        continue
    before = open(hp, encoding="utf-8").read()

    # The generator writes in place, so keep a copy and restore it. A
    # freshness check that leaves the tree dirty is a freshness check nobody
    # runs twice.
    with tempfile.TemporaryDirectory() as tmp:
        keep = os.path.join(tmp, "keep.h")
        shutil.copy2(hp, keep)
        try:
            r = subprocess.run(argv, cwd=root, capture_output=True, text=True)
            if r.returncode != 0:
                why = [l.strip() for l in (r.stdout + r.stderr).splitlines()
                       if "ERROR" in l or "FAILED" in l]
                print(f"SKIPPED: {header} cannot be regenerated here, so freshness")
                print(f"         is unverifiable. The generator refuses to emit while")
                print(f"         content errors stand:")
                for l in why[:4]:
                    print(f"           {l}")
                if len(why) > 4:
                    print(f"           ... and {len(why) - 4} more")
                skipped += 1
                shutil.copy2(keep, hp)
                continue
            after = open(hp, encoding="utf-8").read()
        finally:
            shutil.copy2(keep, hp)

    if before != after:
        bad += 1
        print(f"STALE: {header} does not match {source}")
        diff = difflib.unified_diff(
            before.splitlines(), after.splitlines(),
            fromfile=f"{header} (committed)", tofile=f"{header} (regenerated)",
            lineterm="", n=1,
        )
        for i, line in enumerate(diff):
            if i > 20:
                print("  ...")
                break
            print("  " + line)
        print(f"  fix: {' '.join(os.path.basename(a) for a in argv[1:])}")
    else:
        print(f"fresh: {header} matches {source}")

# The Rust fork generates one too, and guards it in its own suite
# (`archive::tests::the_generated_c_header_is_up_to_date`), because the
# generator is a Rust binary and running it from here would mean building
# that crate. Named so the coverage is visible rather than assumed.
print("note: exengine/archive_gen.h is guarded by games/blackstar-rs's own tests")

if bad:
    print(f"FAILED: {bad} generated file(s) do not match their source.")
    sys.exit(1)
if skipped:
    print(f"gen freshness: {len(TARGETS) - skipped} checked, {skipped} unverifiable here.")
    print("  an environment with the full asset set will check the rest.")
    sys.exit(0)
print(f"gen freshness OK ({len(TARGETS)} generated file(s))")
