#!/usr/bin/env python3
"""board_gestures_parity_check.py — rusty-board's GTK app and the
board-gestures crate must decode a swipe the same way, because nothing else
makes them.

WHY THIS EXISTS

`apps/rusty-board/egui-frontend/Cargo.toml` says, above its dependency line:

    # the SHARED swipe recognizer (same code the GTK app uses — see
    # crates/board-gestures)

The first half is true and the second half is not. The GTK crate does **not**
depend on `board-gestures`; it has its own `src/gestures/swipe.rs` (1143
lines) and `src/gestures/recognizer.rs` (1085 lines), and the crate has its own
copies at 1148 and 1085. Two frontends of one keyboard, two implementations of
the gesture that is the keyboard's headline feature.

As of this checker's first run they AGREE — the diff is entirely the
GTK-decoupling refactor described below, with no behavioural difference. That
is exactly why it is worth pinning now: the copies are in sync, so the checker
starts green for a real reason, and the first drift is the thing it catches
rather than a backlog it inherits.

The failure mode if they drift is the expensive kind. Swipe decoding returns a
plausible word either way — nobody gets a panic or a compile error, the two
frontends just start disagreeing about what the user swiped, and it presents
as "swipe got worse on the egui build" months later. That is the same shape as
`voiceprint_parity_check.py`'s (a fingerprint that keeps returning numbers,
just wrong ones), and this file is deliberately modelled on it.

THE REAL FIX

The GTK app depends on `board-gestures` and deletes its two copies, the way
`apps/snapclone`'s gesture/group/audio crates already moved onto `gesture-kit`,
`snapcore` and `audio-kit`. That is a real change to a crate that cannot be
compiled in the authoring sandbox (GTK4 headers are not installable here), so
it is not something to do blind. This is the cheap guard until then, and it is
recorded in C18_TODO.

WHAT COUNTS AS "THE SAME"

Not byte-identical. The crate exists to be frontend-neutral, so it replaced
`rusty_board::keyboard::Key` with the two fields the algorithm actually reads:

    GTK                          board-gestures
    hitbox.key.label             hitbox.label
    hitbox.key.swipe_target      hitbox.swipe_target
    key: Key::character(l)       label: l.to_string(), swipe_target: true
    crate::gestures::swipe::X    crate::swipe::X

Those four substitutions are applied to the GTK side before comparing, along
with comments and blank lines being stripped — a doc rephrase is not drift.
Everything after that (every threshold, every weight, every branch) must match.

If the decoupling grows a fifth legitimate substitution, add it HERE with a
sentence saying why it is not behavioural. Widening the normaliser to make a
failure go away is how this checker becomes decoration.

USAGE

    python3 maintenance/tools/board_gestures_parity_check.py [repo_root]

Exit 0 and a one-line PASS. Exit 1 with a named cause otherwise: MISSING (a
file is gone — a hard failure, not a skip, because a checker that finds
nothing to compare must not pass; CLAUDE.md §5b) or DRIFTED (both present,
bodies differ after normalisation).
"""
import difflib
import pathlib
import re
import sys

PAIRS = [
    ("apps/rusty-board/src/gestures/swipe.rs",
     "apps/rusty-board/crates/board-gestures/src/swipe.rs"),
    ("apps/rusty-board/src/gestures/recognizer.rs",
     "apps/rusty-board/crates/board-gestures/src/recognizer.rs"),
]

# Every line of a real file must survive normalisation to something; if a pair
# normalises to fewer than this many lines, the reader or the normaliser is
# broken rather than the files being tiny. Asserting the scan found something
# is CLAUDE.md §5b's third habit, and the reason it is here is that a glob or
# a path typo otherwise reports PASS on two empty lists.
MIN_LINES = 400


def strip_noise(text):
    """Drop comments and blank lines. A doc rephrase is not drift."""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    out = []
    for line in text.splitlines():
        line = re.sub(r"//.*", "", line).strip()
        if line:
            out.append(line)
    return out


def decouple(lines):
    """Apply the GTK->frontend-neutral substitutions to the GTK side.

    Listed one per rule so a future reader can see exactly what is being
    forgiven, rather than one clever regex that forgives more than intended.

    Note that two of these turn ONE GTK line into TWO crate lines, and this
    emits both. The first version joined them onto one line and then reported
    a diff against the crate's two — a failure caused entirely by the
    measuring instrument. The tempting fix at that point is to compare with
    whitespace collapsed, which would also stop the checker noticing a
    genuine one-line change; emitting the real shape keeps it strict.
    """
    out = []
    for l in lines:
        l = l.replace("crate::gestures::swipe::", "crate::swipe::")
        # The GTK side imports the type the neutral struct does not have.
        if l == "use crate::keyboard::Key;":
            continue
        # Struct field: `pub key: Key,` -> the two fields it decomposes into.
        if re.fullmatch(r"pub key:\s*Key\s*,", l):
            out.append("pub label: String,")
            out.append("pub swipe_target: bool,")
            continue
        # Literal: `key: Key::character(label),` -> the same two fields.
        m = re.fullmatch(r"key:\s*Key::character\((.*)\)\s*,", l)
        if m:
            out.append(f"label: {m.group(1)}.to_string(),")
            out.append("swipe_target: true,")
            continue
        # Field access through the removed `key` member.
        l = re.sub(r"\.key\.(label|swipe_target)\b", r".\1", l)
        out.append(l)
    return out


def main():
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    failures = []
    checked = 0

    for gtk_rel, kit_rel in PAIRS:
        gtk_p, kit_p = root / gtk_rel, root / kit_rel
        missing = [str(p) for p in (gtk_p, kit_p) if not p.is_file()]
        if missing:
            failures.append(
                f"MISSING: {', '.join(missing)}\n"
                "  A copy was moved, renamed or deleted. If the GTK app now\n"
                "  DEPENDS on board-gestures, that is the real fix landing and\n"
                "  this checker should be deleted rather than repaired."
            )
            continue

        a = decouple(strip_noise(gtk_p.read_text(encoding="utf-8", errors="replace")))
        b = strip_noise(kit_p.read_text(encoding="utf-8", errors="replace"))

        if len(a) < MIN_LINES or len(b) < MIN_LINES:
            failures.append(
                f"MISSING: {gtk_rel} normalised to {len(a)} lines and "
                f"{kit_rel} to {len(b)}, both expected >= {MIN_LINES}.\n"
                "  Nothing was compared. Treated as a failure on purpose: a\n"
                "  checker that silently compares two empty lists passes."
            )
            continue

        checked += 1
        if a != b:
            diff = "\n".join(
                list(difflib.unified_diff(a, b, gtk_rel, kit_rel, lineterm=""))[:40]
            )
            failures.append(
                f"DRIFTED: {gtk_rel}\n     vs  {kit_rel}\n{diff}\n"
                "  These two decode the same user gesture for two frontends of\n"
                "  the same keyboard. Port the change to both, or land the real\n"
                "  fix (GTK depends on the crate) and delete this checker."
            )

    if failures:
        print("board_gestures_parity_check: FAIL")
        for f in failures:
            print("\n" + f)
        return 1

    print(f"board_gestures_parity_check: PASS — {checked} file pair(s) match "
          "after the documented decoupling substitutions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
