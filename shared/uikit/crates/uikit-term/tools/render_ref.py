#!/usr/bin/env python3
"""Render testdata/vt/*.bin through reference emulators and write .expect files.

Two oracles, deliberately.

**tmux** is primary for text and cursor. It is a real terminal emulator that a
lot of people drive with real programs every day, and it is the only thing
available here that models the right margin the way xterm does.

**pyte** is primary for per-cell style, because tmux's `capture-pane -e` gives
back SGR escapes that would have to be re-parsed by the very kind of parser
under test, and pyte hands over structured attributes instead.

Neither is gospel, so they are also run AGAINST EACH OTHER on text and cursor,
and a disagreement is a hard error unless the fixture appears in DIVERGENT with
a reason. Three do, and in every one of them tmux is right and pyte is missing
a feature -- which is exactly why the expectations are not generated
from pyte alone. `save_restore` is the instructive one: pyte ignores ESC 7 /
ESC 8, and so does this crate, so a pyte-generated expectation would have
recorded the bug as correct and passed.

Generation needs tmux and pyte; the Rust test that consumes the output needs
neither. That is the point of checking the .expect files in.

Usage:  python3 tools/render_ref.py [name ...]     (default: all .bin present)
        pip install pyte --break-system-packages
        apt-get install -y tmux

Format, one file per capture:
    cols <n>
    rows <n>
    cursor <row> <col>
    (blank)
     0|<row 0 text, trailing blanks trimmed>
     0#<col>:<len>:<fg>/<bg>/<attrs> ...
    ...

Colours are written as PROTOCOL values (i<index> for a palette entry,
rgb:rrggbb for a direct colour), never as rendered pixels: which colour the
program ASKED for is the thing both emulators must agree on, while the palette
each one paints with is a theming choice and is allowed to differ.
"""
import os
import subprocess
import sys
import time

import pyte

HERE = os.path.dirname(os.path.abspath(__file__))
VT = os.path.join(HERE, "..", "testdata", "vt")

COLS, ROWS = 80, 24

# Fixtures where the two oracles disagree, with which one is believed and why.
# Anything not listed here must agree, or generation fails.
DIVERGENT = {
    "margin_bs": "pyte has no deferred-wrap state, so it backspaces from column "
                 "79 instead of from the pending position past the last column. "
                 "tmux (and xterm) land on the last column; tmux believed.",
    "margin_wide": "pyte gives every character width 1, so a double-width glyph "
                   "does not wrap as a unit. tmux believed.",
    "save_restore": "pyte implements neither ESC 7 / ESC 8 nor CSI s / CSI u. "
                    "tmux believed.",
    "alt_screen": "pyte has no alternate screen: it keeps writing to the one "
                  "buffer, so the pager's output stays on screen after the "
                  "switch back and the shell's is gone. tmux believed.",
    "alt_screen_47": "same — pyte has no alternate screen. tmux also shows the "
                     "cursor NOT being restored by ?47l, which is the whole "
                     "reason ?1049 exists. tmux believed.",
    "alt_screen_nested": "same — pyte has no alternate screen. tmux believed.",
}

# pyte reports the eight base colours by name and everything else -- including
# every 256-palette entry -- as resolved hex, so the index is not recoverable
# from its output directly. Inverting the standard xterm palette gets it back;
# anything outside that palette really is a direct colour and is written as one.
NAMED = {
    "black": 0, "red": 1, "green": 2, "brown": 3, "yellow": 3,
    "blue": 4, "magenta": 5, "cyan": 6, "white": 7,
    "brightblack": 8, "brightred": 9, "brightgreen": 10, "brightbrown": 11,
    "brightyellow": 11, "brightblue": 12, "brightmagenta": 13,
    "brightcyan": 14, "brightwhite": 15,
}

# The xterm 256-colour palette: 16 system colours, a 6x6x6 cube, then 24 greys.
_SYSTEM = [
    (0x00, 0x00, 0x00), (0xCD, 0x00, 0x00), (0x00, 0xCD, 0x00), (0xCD, 0xCD, 0x00),
    (0x00, 0x00, 0xEE), (0xCD, 0x00, 0xCD), (0x00, 0xCD, 0xCD), (0xE5, 0xE5, 0xE5),
    (0x7F, 0x7F, 0x7F), (0xFF, 0x00, 0x00), (0x00, 0xFF, 0x00), (0xFF, 0xFF, 0x00),
    (0x5C, 0x5C, 0xFF), (0xFF, 0x00, 0xFF), (0x00, 0xFF, 0xFF), (0xFF, 0xFF, 0xFF),
]
_LEVELS = (0, 95, 135, 175, 215, 255)


def _xterm256():
    table = {}
    for i, rgb in enumerate(_SYSTEM):
        table.setdefault(rgb, i)
    for r in range(6):
        for g in range(6):
            for b in range(6):
                idx = 16 + 36 * r + 6 * g + b
                table.setdefault((_LEVELS[r], _LEVELS[g], _LEVELS[b]), idx)
    for i in range(24):
        v = 8 + 10 * i
        table.setdefault((v, v, v), 232 + i)
    return table


XTERM256 = _xterm256()


def canon_color(value):
    """A pyte colour -> the protocol vocabulary the Rust dumper prints."""
    if value == "default":
        return "default"
    key = str(value).lower()
    if key in NAMED:
        return "i%d" % NAMED[key]
    if len(key) == 6 and all(c in "0123456789abcdef" for c in key):
        rgb = (int(key[0:2], 16), int(key[2:4], 16), int(key[4:6], 16))
        if rgb in XTERM256:
            return "i%d" % XTERM256[rgb]
        return "rgb:" + key
    return "rgb:" + key


def style_key(cell):
    attrs = ""
    if cell.bold:
        attrs += "b"
    if cell.reverse:
        attrs += "r"
    return "%s/%s/%s" % (canon_color(cell.fg), canon_color(cell.bg), attrs or "-")


def style_runs(screen, row):
    runs = []
    for col in range(COLS):
        key = style_key(screen.buffer[row][col])
        if runs and runs[-1][2] == key:
            runs[-1][1] += 1
        else:
            runs.append([col, 1, key])
    return " ".join("%d:%d:%s" % (c, n, k) for c, n, k in runs)


def render_pyte(path):
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    with open(path, "rb") as fh:
        stream.feed(fh.read())
    lines = [screen.display[r].rstrip() for r in range(ROWS)]
    runs = [style_runs(screen, r) for r in range(ROWS)]
    return lines, runs, screen.cursor.y, screen.cursor.x


def _tmux(*args):
    return subprocess.run(["tmux"] + list(args), capture_output=True, text=True)


def render_tmux(path, session="vtref"):
    """Feed the capture to a real tmux pane and read the pane back.

    `stty -opost` matters: without it the pane's line discipline rewrites every
    bare LF as CRLF before the emulator sees it, which quietly turns the margin
    fixtures into a test of something else entirely. That mistake was made once
    here and it made the margin behaviour look correct.
    """
    _tmux("kill-session", "-t", session)
    cmd = "stty -opost; cat %s; sleep 60" % path
    res = _tmux("new-session", "-d", "-s", session,
                "-x", str(COLS), "-y", str(ROWS), cmd)
    if res.returncode != 0:
        raise RuntimeError("tmux new-session failed: %s" % res.stderr.strip())
    time.sleep(0.6)
    cur = _tmux("display-message", "-p", "-t", session,
                "#{cursor_y} #{cursor_x}").stdout.split()
    pane = _tmux("capture-pane", "-p", "-t", session).stdout
    _tmux("kill-session", "-t", session)
    lines = [l.rstrip() for l in pane.split("\n")]
    # capture-pane omits trailing blank rows; pad back to full height.
    lines = (lines + [""] * ROWS)[:ROWS]
    return lines, int(cur[0]), int(cur[1])


def main():
    names = sys.argv[1:] or sorted(
        f[:-4] for f in os.listdir(VT) if f.endswith(".bin")
    )
    failed = []
    for name in names:
        path = os.path.join(VT, name + ".bin")
        p_lines, p_runs, p_y, p_x = render_pyte(path)
        t_lines, t_y, t_x = render_tmux(path)

        agree = (p_lines == t_lines) and (p_y, p_x) == (t_y, t_x)
        if not agree and name not in DIVERGENT:
            failed.append(name)
            print("%-24s ORACLES DISAGREE and no reason is recorded" % name)
            print("    pyte cursor=%d,%d  tmux cursor=%d,%d" % (p_y, p_x, t_y, t_x))
            for i, (a, b) in enumerate(zip(t_lines, p_lines)):
                if a != b:
                    print("    row%-2d tmux=%r" % (i, a))
                    print("          pyte=%r" % b)
            continue
        if agree and name in DIVERGENT:
            failed.append(name)
            print("%-24s listed in DIVERGENT but the oracles now AGREE -- "
                  "delete the entry" % name)
            continue

        out = ["cols %d" % COLS, "rows %d" % ROWS,
               "cursor %d %d" % (t_y, t_x), ""]
        for i, text in enumerate(t_lines):
            out.append("%2d|%s" % (i, text))
            out.append("%2d#%s" % (i, p_runs[i]))
        with open(os.path.join(VT, name + ".expect"), "w") as fh:
            fh.write("\n".join(out) + "\n")
        mark = "  (divergent, tmux believed)" if name in DIVERGENT else ""
        print("%-24s cursor=%d,%d%s" % (name, t_y, t_x, mark))

    if failed:
        sys.exit("\n%d fixture(s) unresolved: %s" % (len(failed), ", ".join(failed)))


if __name__ == "__main__":
    main()
