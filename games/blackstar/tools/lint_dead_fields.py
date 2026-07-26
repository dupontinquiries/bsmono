#!/usr/bin/env python3
"""lint_dead_fields.py -- catch the recurring "defined-but-never-applied" bug.

Several config tables in this codebase have shipped fields that were set in the data
table but never READ in the game (dead class hp_mul, chassis xform_dur_mul,
greeble_boost, ...). Each was found by hand. This lints for them automatically: for
each config struct, it extracts the field names, then checks whether each field is
ever accessed via `->field` or `.field` anywhere in the C sources. A field with zero
accesses is set-but-never-used -- almost certainly dead data.

Exits non-zero if any un-whitelisted dead field is found, so it can gate a build.
Usage: python3 tools/lint_dead_fields.py [--root .]
"""
import os, re, sys, glob

STRUCTS = {
    "ex_chassis_info_t": "engine_src/exengine/chassis.h",
    "ex_class_info_t":   "engine_src/exengine/pclass.h",
    "ex_sig_stats_t":    "engine_src/exengine/signature.h",
    "ex_sig_combat_t":   "engine_src/exengine/signature.h",
}
# fields that are legitimately name-only / structural (accessed indirectly, or purely
# descriptive) -- kept out of the dead list on purpose.
WHITELIST = {"name", "tagline"}

def struct_fields(text, name):
    """Return the field identifiers of `typedef struct { ... } name;`."""
    m = re.search(r"typedef\s+struct\s*\{([^{}]*)\}\s*" + re.escape(name) + r"\s*;", text, re.S)
    if not m:
        return []
    body = re.sub(r"/\*.*?\*/", "", m.group(1), flags=re.S)   # strip block comments
    body = re.sub(r"//.*", "", body)                          # strip line comments
    fields = []
    for line in body.split(";"):
        line = line.strip()
        if not line:
            continue
        # drop the type; keep the comma-separated declarators
        parts = line.split()
        if len(parts) < 2:
            continue
        decl = " ".join(parts[1:])            # everything after the type
        for tok in decl.split(","):
            tok = tok.strip().lstrip("*").strip()
            tok = re.sub(r"\[.*?\]", "", tok)  # drop array dims
            tok = tok.strip("* ")
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", tok):
                fields.append(tok)
    return fields

def main():
    root = "."
    args = sys.argv[1:]
    if "--root" in args:
        root = args[args.index("--root") + 1]

    # gather all C sources (game + engine modules), plus their headers for reference
    srcs = glob.glob(os.path.join(root, "engine_src", "*.c")) + \
           glob.glob(os.path.join(root, "engine_src", "exengine", "**", "*.c"), recursive=True)
    blob = ""
    for s in srcs:
        try: blob += open(s, errors="ignore").read()
        except OSError: pass

    dead = []
    checked = 0
    for sname, hpath in STRUCTS.items():
        try:
            htext = open(os.path.join(root, hpath), errors="ignore").read()
        except OSError:
            print(f"  (skip {sname}: {hpath} not found)"); continue
        for f in struct_fields(htext, sname):
            if f in WHITELIST:
                continue
            checked += 1
            # a real read/write is `->field` or `.field` (word-bounded)
            if re.search(r"(->|\.)" + re.escape(f) + r"\b", blob):
                continue
            dead.append((sname, f))

    print(f"dead-field lint: checked {checked} fields across {len(STRUCTS)} config structs")
    for sname, f in dead:
        print(f"  DEAD: {sname}.{f} is set in its data table but never read via ->/. in any source")
    if dead:
        print(f"FAILED: {len(dead)} dead field(s).")
        return 1
    print("OK: every config field is applied somewhere.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
