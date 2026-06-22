#!/usr/bin/env python3
"""check_shaders.py — surface the stale-.spv trap (see SESSION_HANDOFF.md s1).

Lists every shader in build.rs's SHADERS list and reports whether its compiled
artifact exists and looks fresh. mtime-based freshness works in a live checkout
(not inside a zip). Run from the engine crate dir, or pass --crate <path>.

  python3 handoff/check_shaders.py [--crate crates/unendlich-engine]

Exit nonzero if any listed shader is missing its .spv or the source is newer
than the .spv (i.e. an edit that hasn't been recompiled).
"""
import os, re, sys

def main(argv):
    crate = "crates/unendlich-engine"
    if "--crate" in argv:
        crate = argv[argv.index("--crate") + 1]
    build_rs = os.path.join(crate, "build.rs")
    if not os.path.exists(build_rs):
        print(f"build.rs not found at {build_rs}; pass --crate <path>"); return 2
    txt = open(build_rs).read()
    # SHADERS list entries look like: "src/shaders/foo.comp",
    shaders = re.findall(r'"(src/shaders/[^"]+\.(?:comp|frag|vert))"', txt)
    if not shaders:
        print("no SHADERS entries parsed — check build.rs format"); return 2
    # build.rs writes .spv to ../../assets/shaders relative to the crate.
    out_dir = os.path.join(crate, "..", "..", "assets", "shaders")
    problems = 0
    print(f"{'shader':40} {'.spv':8} {'fresh':6}")
    for s in shaders:
        src = os.path.join(crate, s)
        name = os.path.basename(s) + ".spv"
        spv = os.path.join(out_dir, name)
        has_spv = os.path.exists(spv)
        fresh = "n/a"
        if has_spv and os.path.exists(src):
            fresh = "yes" if os.path.getmtime(spv) >= os.path.getmtime(src) else "STALE"
        flag = ""
        if not has_spv:
            flag = "  <- .spv MISSING (build.rs will compile if glslang present)"
            problems += 1
        elif fresh == "STALE":
            flag = "  <- SOURCE NEWER THAN .spv — force_recompile_shader.sh it!"
            problems += 1
        print(f"{os.path.basename(s):40} {'yes' if has_spv else 'NO':8} {fresh:6}{flag}")
    print(f"\n{problems} potential stale/missing artifact(s).")
    return 1 if problems else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv))
