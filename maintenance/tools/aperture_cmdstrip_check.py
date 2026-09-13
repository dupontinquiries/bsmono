#!/usr/bin/env python3
"""aperture_cmdstrip_check.py — the command strip pins commands by LABEL, and
nothing makes the two lists agree.

WHY THIS EXISTS

`state.rs::default_cmd_strip` pins three commands onto the strip as bare
string literals:

    "File: Save"
    "Find: In Project… (Ctrl+Shift+F)"
    "AI: Complete now"

`state.rs::run_strip_command` then resolves a strip id by searching
`palette_ui::command_list()` for a label equal to it. That resolution is
single-sourced and correct — the strip does NOT reimplement any command, it
delegates, and the comment there explains why the label is the id (the
palette has no separate command id, so inventing one would be a second
naming scheme).

The gap is one step earlier. The three PINNED labels are written in a
different file from the labels they must match, with no compile-time link
between them. Reword a palette entry — add a shortcut hint, fix the ellipsis,
change "AI:" to "Assistant:" — and the pin stops resolving. Nothing fails to
compile, no test breaks, and the button goes on rendering; pressing it takes
the `else` branch in `run_strip_command` and says "that command is no longer
available". A button that is visibly there and quietly dead.

All three resolve as of this checker's first run, so it starts green for a
real reason and the first drift is what it catches, rather than inheriting a
backlog. Same shape and same reasoning as
`board_gestures_parity_check.py`.

WHY A CHECKER AND NOT A REFACTOR

The obvious alternative is to give commands stable ids and pin by id. That is
a real change to `palette_ui::Cmd`, `command_list`, `matches`,
`run_strip_command` and `uikit_egui::cmdstrip`, to remove a hazard that has
not fired yet — and `run_strip_command`'s own comment already weighed a
second naming scheme and declined it, deliberately. This pins the decision
that was made instead of overturning it for the cost of a grep.

USAGE

    python3 maintenance/tools/aperture_cmdstrip_check.py [repo_root]

Exit 0 with a PASS line; exit 1 naming each pinned label with no matching
palette entry, and the closest palette labels, since the usual cause is a
reworded entry rather than a deleted one.
"""
import difflib
import pathlib
import re
import sys

PALETTE = "apps/aperture/crates/aperture-egui/src/palette_ui.rs"
STATE = "apps/aperture/crates/aperture-egui/src/state.rs"

# A strip with no pins would pass every comparison below, so the count is
# asserted rather than assumed — CLAUDE.md §5b's third habit, and the reason
# this file cannot report a false clean if `default_cmd_strip` is emptied or
# the regex stops matching.
MIN_PINS = 1


def palette_labels(text):
    """Every label `command_list()` offers."""
    start = text.index("pub fn command_list")
    body = text[start:]
    end = body.index("\npub fn ", 10)
    body = body[:end]
    return set(re.findall(r'"([^"]+)"', body))


def pinned_labels(text):
    """Every label `default_cmd_strip()` pins."""
    start = text.index("fn default_cmd_strip")
    body = text[start:]
    body = body[: body.index("\n}")]
    return re.findall(r'"([^"]+)"', body)


def main():
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    try:
        pal = (root / PALETTE).read_text(encoding="utf-8")
        st = (root / STATE).read_text(encoding="utf-8")
    except OSError as e:
        print(f"aperture_cmdstrip_check: FAIL — cannot read a source file: {e}")
        return 1

    try:
        labels = palette_labels(pal)
        pins = pinned_labels(st)
    except ValueError as e:
        print(
            "aperture_cmdstrip_check: FAIL — could not locate command_list or "
            f"default_cmd_strip ({e}). One was renamed or moved; this checker "
            "must be repointed rather than deleted, or the drift it guards "
            "goes unwatched."
        )
        return 1

    if len(pins) < MIN_PINS:
        print(
            f"aperture_cmdstrip_check: FAIL — found {len(pins)} pinned labels, "
            f"expected at least {MIN_PINS}. Nothing was compared."
        )
        return 1

    missing = [p for p in pins if p not in labels]
    if missing:
        print(
            f"aperture_cmdstrip_check: FAIL — {len(missing)} of {len(pins)} "
            "pinned strip commands no longer match a palette label\n"
        )
        for m in missing:
            print(f"  pinned:  {m!r}")
            for near in difflib.get_close_matches(m, labels, n=2, cutoff=0.6):
                print(f"    close: {near!r}")
            print(
                "    This button still renders and does nothing —"
                " run_strip_command falls through to"
                ' "that command is no longer available".'
            )
        print(
            "\n  Fix by updating the literal in default_cmd_strip to the new "
            "palette label, or by removing the pin if the command is gone."
        )
        return 1

    print(
        f"aperture_cmdstrip_check: PASS — all {len(pins)} pinned strip "
        f"commands resolve against {len(labels)} palette labels"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
