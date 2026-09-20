#!/usr/bin/env python3
"""An egui app that draws a command palette draws it with the kit's surface.

## Why

`uikit_egui::command_bar::show` is the palette surface since c19: on a compact or
touch surface it sits on the bottom edge with the input last and the best match
directly above it; on a desk it is the usual top-centre card. Before that every
app drew its own, and every one drew the desktop shape at every size - so on a
phone the input was at the far end of the screen from the thumb and the keyboard,
and the best match furthest of all.

The surface also carries three fixes that each cost a session to find, and a
hand-rolled palette has to find them again:

  * rows need `Button::truncate` (the widget overrides a LayoutJob's own wrap
    settings, so a long detail wraps every row to three lines);
  * the click that OPENS the palette is, on that frame, a click outside it;
  * rows need `clicked_by(Primary)`: egui's arrow-key focus navigation runs
    before the arrows can be consumed, and Enter then "clicks" whichever row took
    focus instead of the one under the cursor.

Nothing notices when the next palette is written the old way. This does.

## The rule

A crate that depends on egui and whose code DRAWS a palette must call
`command_bar::show` somewhere in its own source, or be listed in KNOWN_MISSES
with the reason it has not moved.

"Draws a palette" is a heuristic, stated so it can be argued with: a source file
whose CODE (comments removed) has an identifier or string containing `palette`
in the command sense - `command palette`, `palette_ui`, `palette_open`,
`open_palette`, `CommandPalette`, `cmd_palette` - AND builds an overlay or a text
input in the same file (`egui::Window::new`, `egui::Area::new`, `TextEdit::`,
`text_field::`). Colour palettes (`apply_palette`, `PaletteColors`,
`palette_for`) do not count. It over-reports rather than under-reports; an entry
that turns out not to draw one belongs in NOT_A_PALETTE with a line saying why.

## What it cannot see

Whether a crate that calls `show` ALSO still draws an old one beside it, and
anything about how the palette behaves. The surface's behaviour is pinned by
`command_bar`'s own `surface_tests`.

usage: palette_surface_check.py [repo-root]
"""
import os
import re
import sys

ROOT = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")

# Crates found drawing their own palette when this checker was written. Each port
# is mostly deletion (vidkit-player: +63 / -176) but also takes the kit's
# clamp-not-wrap cursor and its arrow/Enter behaviour, so each needs looking at
# on a screen once - which is why they were not converted blind in one pass.
KNOWN_MISSES = {
    "apps/aperture/crates/aperture-egui":
        "src/palette_ui.rs, with a Mode of its own (commands / files / symbols). The one "
        "most worth moving: it already pairs the palette with cmdstrip's hold button, "
        "so on a phone it is reachable AND the wrong way up.",
    "apps/beatspace":
        "src/arrangepage.rs::palette_ui. Already takes accept/ghost/move_cursor/MAX_ROWS "
        "from command_bar, so the port is the drawing only. Needs alsa + opus to build.",
    "apps/snapclone/egui-frontend":
        "src/lib.rs, `palette_open`. command_bar's module doc cites THIS palette's weak "
        "scrim as the reason the kit decided what the surface is; it has not come back.",
    "apps/studio/crates/studio-app":
        "src/lib.rs, an egui::Window titled \"Command palette\" with its own open flag.",
    "apps/tomescope":
        "src/main.rs, `command_palette_open`, opened from a ⌘K button.",
    "apps/voiceterm/egui-frontend":
        "src/bin/voiceterm_egui.rs::palette_ui, animated by `open_t` and tied to "
        "palette_flow + voice_popup. The largest port and the one with the most to lose "
        "by being done blind.",
}

# Matched the heuristic, looked at, not a command palette.
NOT_A_PALETTE = {
    "apps/logscope/crates/logscope-egui":
        "tabs/chat_draw.rs: `cmd_palette` is the id of a zsh-style completion grid for "
        "`:` commands inside the chat composer - inline, not an overlay with its own input.",
}

# The surface's first consumers. If the scan stops finding them, the scan is
# broken, and every "pass" below it would mean nothing.
MUST_FIND_USING_SHOW = ("apps/stoat/crates/gui-egui", "apps/vidkit-player")

LINE_COMMENT = re.compile(r"^\s*//.*$", re.M)
PALETTE_WORD = re.compile(
    r"command[ _-]?palette|commandpalette|palette_ui|palette_open|open_palette|"
    r"cmd_palette|palette\.open\(|toggle_palette|show_palette",
    re.I,
)
OVERLAY = re.compile(r"egui::Window::new|egui::Area::new|Window::new\(|Area::new\(|TextEdit::|text_field::")
USES_SHOW = re.compile(r"command_bar::show\s*\(")


def code_only(text):
    """Line comments removed - a module doc that DISCUSSES palettes must not make
    its crate one. (Same trap font_install_check.py records falling into.)"""
    return LINE_COMMENT.sub("", text)


def egui_crates():
    out = []
    for base in ("apps", "games", "system", "tools"):
        for dirpath, dirnames, filenames in os.walk(os.path.join(ROOT, base)):
            dirnames[:] = [d for d in dirnames if d not in {"target", ".git", "node_modules"}]
            if "Cargo.toml" not in filenames or not os.path.isdir(os.path.join(dirpath, "src")):
                continue
            try:
                manifest = open(os.path.join(dirpath, "Cargo.toml"), errors="replace").read()
            except OSError:
                continue
            if re.search(r"^\s*(egui|eframe|uikit-egui)\s*[=.]", manifest, re.M):
                out.append(os.path.relpath(dirpath, ROOT))
    return sorted(out)


def own_sources(crate_dir):
    """.rs files of this crate, not of crates nested inside it."""
    base = os.path.join(ROOT, crate_dir)
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [
            d for d in dirnames
            if d not in {"target", ".git"} and not (dirpath != base and os.path.exists(os.path.join(dirpath, d, "Cargo.toml")))
            and not (dirpath == base and d != "src" and os.path.exists(os.path.join(dirpath, d, "Cargo.toml")))
        ]
        for f in filenames:
            if f.endswith(".rs"):
                yield os.path.join(dirpath, f)


def inspect(crate_dir):
    """(files that draw a palette, does the crate call command_bar::show)."""
    drawers, uses_show = [], False
    for path in own_sources(crate_dir):
        try:
            code = code_only(open(path, errors="replace").read())
        except OSError:
            continue
        if USES_SHOW.search(code):
            uses_show = True
        if PALETTE_WORD.search(code) and OVERLAY.search(code):
            drawers.append(os.path.relpath(path, ROOT))
    return drawers, uses_show


def main():
    crates = egui_crates()
    drawing, using = {}, set()
    for c in crates:
        files, uses = inspect(c)
        if uses:
            using.add(c)
        if files:
            drawing[c] = files

    lost = [c for c in MUST_FIND_USING_SHOW if c not in using]
    if lost:
        print(f"palette_surface_check: BROKEN SCAN - did not find command_bar::show in {lost}; "
              "nothing below can be trusted")
        return 2

    misses = sorted(c for c in drawing if c not in using and c not in NOT_A_PALETTE)
    new = [m for m in misses if m not in KNOWN_MISSES]
    fixed = [m for m in KNOWN_MISSES if m not in misses]

    print(f"scanned {len(crates)} egui crates: {len(using)} draw through command_bar::show, "
          f"{len(misses)} draw their own")
    for m in sorted(KNOWN_MISSES):
        if m in misses:
            print(f"  known miss: {m}\n      {KNOWN_MISSES[m]}")
    for m in fixed:
        print(f"  FIXED or gone, delete from KNOWN_MISSES: {m}")
    for m in new:
        print(f"  NEW: {m} draws a palette without command_bar::show")
        for f in drawing[m][:4]:
            print(f"        {f}")
    if new or fixed:
        print("palette_surface_check: FAIL")
        return 1
    print("palette_surface_check: PASS (known misses are a queue, not a pardon)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
