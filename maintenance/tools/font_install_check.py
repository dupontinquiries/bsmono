#!/usr/bin/env python3
"""Every egui app must reach `uikit_egui::fonts::install`, or its glyphs are boxes.

# Why this exists

`shared/uikit/crates/uikit-egui/examples/glyph_audit.rs` measured that 132 of
the characters this tree draws had no glyph in egui's bundled fonts — `→` at
1376 uses, then `✓`, `✗`, `●`. Commit 7098bc4a shipped subsetted Noto faces
that fix 89 of them, installed from `uikit_egui::apply_palette`, on the
reasoning that a font every app must remember to install is how the problem
happened in the first place.

That reasoning was right and the coverage was not. `apply_palette` fixes an app
only if the app CALLS it, and switcheroo did not — it was the one egui app in
the fleet with no `uikit-egui` dependency at all. Its `★`, `☆` and `↻` are
exactly the glyphs the defaults lack, so they rendered as boxes there while
working everywhere else (fixed in 51e910c8).

Nothing flagged that. It was found by reading switcheroo's colours for an
unrelated reason. The symptom — boxes for characters that are fine in the next
app — is close to unfindable by inspection, which is what makes it worth a
checker rather than a note.

# What counts as reaching it

Any of `apply_palette`, `apply_prefs` (which calls it) or a direct
`fonts::install`. An app that themes itself some other way does NOT count: a
local palette gives it colours and no fonts, which is exactly the state
logscope is in below.

# What this cannot see

Whether the call is on a path that actually runs. A frontend that calls
`apply_palette` behind a feature nobody builds passes here and boxes in the
field. Reachability needs the running binary; this is the cheap check that
catches the case that has actually occurred twice.

Usage:  python3 maintenance/tools/font_install_check.py [repo-root]
"""
import os
import re
import sys

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")

# Reaching `fonts::install`, directly or through the palette.
REACHES = re.compile(r"\b(apply_palette|apply_prefs|palette_colors_ctx|fonts::install)\b")

# Crates that draw with egui but are NOT app frontends, so the fonts are the
# embedding app's business rather than theirs.
NOT_A_FRONTEND = {
    "shared/uikit/crates/uikit-egui",     # the kit itself
    "shared/uikit/crates/uikit-term",     # a widget, embedded
    "shared/uikit/crates/uikit-gesture",
    "shared/uikit/crates/uikit-web",
    "apps/settings/egui-frontend",        # role = "lib" in the manifest
    "system/snapclone/camera-bridge",     # a probe binary, no chrome
    "shared/scene-kit",                   # a library, embedded by games
    "apps/aperture/crates/aperture-scene",
    "apps/fao-codec/fao_engine_capture",
}

# Known misses, with what each costs. An entry here is a debt, not an
# exemption: the checker stays green so it is worth running, and the list is
# the worklist. Same shape as uikit-term's KNOWN_GAPS, and for the same
# reason — a checker that fails on arrival gets ignored, and then it is not a
# checker.
KNOWN_MISSES = {
    "apps/logscope/crates/logscope-egui":
        "themes itself through its own `theme::install_palette` rather than the "
        "kit's, so it has colours and no fonts. The nearest fix is one call, but "
        "logscope draws the most symbols of anything here (29 surfaces) and the "
        "change wants looking at on a screen.",
    "apps/av1tuner/av1tuner_app":
        "no uikit-egui dependency and no theming at all — default egui. Same "
        "shape switcheroo was in before 51e910c8.",
    "apps/cadscope/crates/cadscope-egui":
        "no uikit-egui dependency; grafted from a fork (C12-74) and never put on "
        "the kit.",
    # The rest, found by this checker on its first run. Each is one call in the
    # app's startup, but each also needs looking at once on a screen — adding
    # `apply_palette` to an app that currently themes itself by hand changes
    # every colour on it, which is a bigger change than "install the fonts" and
    # should not be made blind across ten apps in one pass.
    "apps/ambient-learner": "the Qt-era shell; the egui frontend beside it is a separate entry.",
    "apps/beatspace": "no palette call.",
    "apps/settings-app": "bsettings; no palette call.",
    "apps/snapclone/camera-bridge": "a probe UI; low glyph count, low priority.",
    "apps/snapshot-cam": "no palette call.",
    "games/road-warrior/shell": "no palette call.",
    "games/unendlich/crates/unendlich-dash": "telemetry dashboard; also the crate "
        "eframe_backend_check flags (C14_TODO §6).",
}


def crates_with_eframe():
    """Directories whose Cargo.toml depends on eframe — i.e. own a window."""
    out = []
    for base in ("apps", "games", "system", "shared", "tools"):
        for dirpath, dirnames, filenames in os.walk(os.path.join(ROOT, base)):
            dirnames[:] = [d for d in dirnames if d not in {"target", ".git", "node_modules"}]
            if "Cargo.toml" not in filenames:
                continue
            try:
                text = open(os.path.join(dirpath, "Cargo.toml"), errors="replace").read()
            except OSError:
                continue
            # `eframe` as a dependency, not merely the word in a comment.
            if not re.search(r"^\s*eframe\s*[=.]", text, re.M):
                continue
            # A WORKSPACE ROOT that declares eframe for its members is not
            # itself a frontend, and counting it reports the same app twice
            # under two names — which is how a checker's output stops being
            # read. A crate has code of its own or it is not one.
            if not os.path.isdir(os.path.join(dirpath, "src")):
                continue
            out.append(os.path.relpath(dirpath, ROOT))
    return sorted(out)


LINE_COMMENT = re.compile(r"^\s*//.*$", re.M)


def code_only(text):
    """Source with line comments removed.

    Load-bearing, and found by breaking the checker: switcheroo's own module
    doc EXPLAINS the `apply_palette` / `fonts::install` relationship, so with
    comments included the file matched on prose and the crate passed with its
    real call deleted. A checker that a comment can satisfy is a checker that
    passes on documentation about the thing it is looking for — which is
    precisely the kind of file the offending apps are most likely to have.
    """
    return LINE_COMMENT.sub("", text)


def reaches_install(crate_dir):
    """Does this crate's OWN source reach the font install?

    Nested crates are excluded, and that is not a detail: `apps/ambient-learner`
    contains `apps/ambient-learner/egui-frontend`, so a plain walk gave the
    parent credit for the child's call and reported the parent fixed when
    nothing in it had changed. The checker's own "FIXED, delete from
    KNOWN_MISSES" arm is what surfaced that — it fired for a crate I had not
    touched.
    """
    root = os.path.join(ROOT, crate_dir)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            d for d in dirnames
            if d != "target"
            and not os.path.exists(os.path.join(dirpath, d, "Cargo.toml"))
        ]
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            try:
                if REACHES.search(code_only(open(os.path.join(dirpath, fn), errors="replace").read())):
                    return True
            except OSError:
                pass
    return False


def main():
    crates = crates_with_eframe()
    # A scan that matched nothing passes every assertion below it.
    if len(crates) < 10:
        print(f"only {len(crates)} eframe crates found under {ROOT} — is the root right?")
        return 2

    missing, ok = [], 0
    for c in crates:
        if c in NOT_A_FRONTEND:
            continue
        if reaches_install(c):
            ok += 1
        else:
            missing.append(c)

    print(f"scanned {len(crates)} eframe crates "
          f"({len(NOT_A_FRONTEND)} not frontends); {ok} install the fonts")

    new = [m for m in missing if m not in KNOWN_MISSES]
    fixed = [m for m in KNOWN_MISSES if m not in missing and m in crates]

    for m in sorted(KNOWN_MISSES):
        if m in missing:
            print(f"  known miss: {m}\n      {KNOWN_MISSES[m]}")

    for m in sorted(new):
        print(f"  MISSING: {m} — draws with egui and never reaches "
              f"fonts::install, so any symbol glyph it uses is a box")

    for m in sorted(fixed):
        print(f"  FIXED, delete from KNOWN_MISSES: {m}")

    # Both directions, so the list cannot rot: a new offender fails, and so
    # does an entry that has quietly been fixed.
    return 1 if (new or fixed) else 0


if __name__ == "__main__":
    sys.exit(main())
