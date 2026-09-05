#!/usr/bin/env python3
"""Capture raw pty output from a real interactive shell into testdata/vt/*.bin.

Why this exists: the grid was mis-tracking the cursor through a themed shell's
prompt redraws (C13_TODO section 19), and no hand-written test found it, because
the sequences a line editor actually emits -- CHA, ECH, deferred wrap at the
right margin, erase-with-implicit-param -- are not what one writes from memory.
So the fixtures are RECORDED, not invented.

Capture is deliberately separate from expectation-generation (render_ref.py):
this half talks to a real shell and is therefore nondeterministic, so a .bin is
recorded ONCE and frozen. render_ref.py is a pure function of that .bin and can
be re-run at will.

Usage:  python3 tools/capture_vt.py [name ...]      (default: all scenarios)
"""
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "testdata", "vt")

# A prompt shaped like the one in the field report: SGR bold+colour, a
# multi-byte glyph, a colour change, a reset. The glyph matters -- it is where
# a byte-indexed cursor and a column-indexed cursor disagree.
BASHRC = (
    "PS1='\\[\\e[1;32m\\]\u279c \\[\\e[36m\\]~\\[\\e[0m\\] '\n"
    "unset PROMPT_COMMAND\n"
    "bind 'set enable-bracketed-paste on' 2>/dev/null\n"
    "bind 'set show-all-if-ambiguous on' 2>/dev/null\n"
)

CR = b"\r"
ESC = b"\x1b"

SCENARIOS = {
    # Baseline: does a prompt + one command + its output land on three rows.
    "prompt_echo": [b"echo hello" + CR],

    # The right-margin case. 76 characters of argument on an 80-column screen
    # means the echoed command line runs past the last column while the line
    # editor is still redrawing it -- deferred wrap and readline's redraw in
    # the same sequence.
    "wrap_long_line": [b"echo " + b"x" * 90 + CR],

    # Line editing: readline redraws the line with CHA/CUB and erases with EL.
    # Home, then forward-word, then kill-to-end, then accept.
    "edit_in_place": [b"echo alpha beta gamma", b"\x01", b"\x05", b"\x17", b"\x17", CR],

    # Backspace over a multi-byte glyph, then over ASCII.
    "backspace_utf8": ["echo \u279c\u279cabc".encode(), b"\x7f\x7f\x7f\x7f", CR],

    # 256-colour and truecolour SGR, which a themed prompt emits constantly.
    "sgr_256_and_truecolor": [
        b"printf '\\033[38;5;33mA\\033[38;2;255;128;0mB\\033[0mC\\n'" + CR
    ],

    # Erase family with implicit parameters, and absolute column addressing.
    "erase_and_cha": [
        b"printf 'abcdefgh\\rXY\\033[K\\n1234\\033[3G_\\033[2K|\\n'" + CR
    ],

    # A tab landing on its column stop after text of a non-multiple width.
    "tab_stops": [b"printf 'ab\\tcd\\tefghijk\\tZ\\n'" + CR],

    # Ctrl-C on a partially typed line: readline emits a fresh prompt below.
    "interrupt_partial_line": [b"echo unfinished", b"\x03", b"echo after" + CR],
}


# ── constructed sequences ───────────────────────────────────────────────────
#
# These are NOT recordings. A shell only reaches the right margin by accident,
# so the margin cases have to be written out. They are still checked against
# pyte exactly like the recordings are -- what makes them different is that the
# bytes are chosen rather than observed, so a mistake here is a mistake about
# what a program WOULD send, which a recording cannot make.
#
# The right margin is where the deferred-wrap ("phantom column") state lives:
# after writing the last column a VT100 leaves the cursor ON that column with a
# pending wrap, rather than moving past it. Every one of these asks what some
# other operation does while that flag is up.
FILL = b"x" * 80          # exactly one 80-column row

SYNTHETIC = {
    # LF at the margin must move down ONE row. If the cursor was parked past
    # the last column instead of on it, the next character wraps a second time
    # and the output lands a row too low.
    "margin_lf": FILL + b"\nZ",
    # CR at the margin clears the pending wrap: Z overwrites column 0 of the
    # SAME row.
    "margin_cr": FILL + b"\rZ",
    # Erase-to-end-of-line while pending: the cursor is on the last column, so
    # that column is erased.
    "margin_el": FILL + b"\x1b[KZ",
    # Erase-to-end-of-display while pending, from the last column of row 0.
    "margin_ed": FILL + b"\x1b[JZ",
    # Backspace cancels the pending wrap and steps back one column.
    "margin_bs": FILL + b"\x08Z",
    # Cursor-forward at the margin has nowhere to go.
    "margin_cuf": FILL + b"\x1b[CZ",
    # A double-width glyph cannot start in the last column: it wraps whole.
    "margin_wide": b"y" * 79 + "世界".encode(),
    # Wrapping onto a row that is itself the last one, so the wrap scrolls.
    "margin_scroll": (b"a" * 80) * 24 + b"TAIL",
    # Absolute column addressing, the opcode a line editor redraws with.
    "cha_and_vpa": b"abcdefgh\x1b[3G_\x1b[5d|\x1b[1;1HQ",
    # Erase-character and insert/delete-character, which readline uses to
    # rewrite the middle of a line without redrawing the tail.
    "ich_dch_ech": b"abcdefgh\x1b[1;3H\x1b[2X\x1b[1;6H\x1b[2P\x1b[1;1H\x1b[3@Z",
    # Insert/delete line and scroll up/down inside a scrolling region.
    "il_dl_region": (
        b"one\r\ntwo\r\nthree\r\nfour\r\nfive"
        b"\x1b[2;4r\x1b[2;1H\x1b[L\x1b[3;1HNEW\x1b[2;1H\x1b[M\x1b[r\x1b[6;1Hend"
    ),
    # Save/restore cursor, both spellings.
    "save_restore": b"abc\x1b7\x1b[3;5Hxyz\x1b8Q\x1b[s\x1b[5;2Hpq\x1b[uR",
    # Charset designation. ESC ( B selects ASCII into G0 and is emitted by
    # ncurses on almost every screen setup; a parser that drops the ESC ( and
    # then treats B as text prints a stray letter into the output.
    "charset_designate": b"\x1b(B\x1b)0hello\x1b(0\x1b(Bworld",
    # DECALN, kept apart from the designators above because it is a different
    # feature that happens to share the ESC-with-intermediate shape: it fills
    # the whole screen with E and homes the cursor. Rare in the wild, but it is
    # the cheapest possible check that an intermediate byte is consumed rather
    # than printed.
    "decaln": b"\x1b#8",
    # The alternate screen: a full-screen program's whole life cycle. Shell
    # output, switch, draw over it, switch back. What must survive is the
    # shell output AND the cursor position, which is the pair a partial
    # implementation gets half right.
    "alt_screen": (
        b"shell one\r\nshell two\r\n\x1b[3;4Hparked"
        b"\x1b[?1049h"
        b"PAGER\r\nsecond line of pager\r\n\x1b[10;1Hbottom"
        b"\x1b[?1049l"
        b"AFTER"
    ),
    # The older spelling, which switches the buffer and NOT the cursor.
    "alt_screen_47": b"abc\x1b[2;3H\x1b[?47hXY\x1b[?47lZ",
    # A redundant enter, which programs do send (a pager under an editor). The
    # second one must not overwrite the parked primary with the alternate.
    "alt_screen_nested": (
        b"keep me\x1b[?1049hfirst alt\x1b[?1049hsecond alt\x1b[?1049l"
    ),
    # Reverse index at the top of the screen scrolls DOWN.
    "reverse_index": b"line1\r\nline2\x1b[1;1H\x1bMtop",
}


# zsh's prompt is themed the same way BASHRC's is (SGR + a glyph), for the same reason: a themed
# prompt is what the original report used, and zle's redraw of a coloured PS1 is a different code
# path from a plain one in some shells' completion/redraw logic.
#
# PROMPT_SP (on by default) is left ON, not turned off: it is what PRODUCES the end-of-line mark --
# a reverse-video placeholder, padding to the terminal width, then \r-and-redraw -- when output
# doesn't end in a newline. That shape is exactly the "themed zsh...different redraw shape"
# C14_TODO section 4 asked for, and bash has no equivalent (it never marks a missing trailing
# newline at all). An earlier draft of this file unsetopt'd PROMPT_SP believing it was extra
# framing around the mark; captured with it off, the mark simply never appeared. Verified the
# other direction too: with PROMPT_SP at its default, the captured bytes are
# ESC[1m ESC[7m # ESC[27m ESC[1m ESC[0m + spaces-to-width + \r SPACE \r\r -- bold+reverse, the #
# placeholder (root-prompt char; regular users get %), then the pad-and-redraw.
ZSHRC = (
    "PS1=$'%{\\e[1;35m%}\\u279c %{\\e[36m%}%1~%{\\e[0m%} '\n"
    "bindkey -e\n"  # emacs keybindings, so edit_in_place-equivalent keys behave like bash's
)


def capture(keys, argv, rcfile_content, rcfile_path, extra_env=None, cols=80, rows=24, settle=0.45):
    rc_dir = os.path.dirname(rcfile_path)
    os.makedirs(rc_dir, exist_ok=True)
    with open(rcfile_path, "w") as fh:
        fh.write(rcfile_content)
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    env.pop("PROMPT_COMMAND", None)
    if extra_env:
        env.update(extra_env)
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(argv[0], argv, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    out = bytearray()

    def drain(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                return False
            if not chunk:
                return False
            out.extend(chunk)
        return True

    drain(1.0)
    for k in keys:
        os.write(fd, k)
        drain(settle)
    os.write(fd, b"exit" + CR)
    drain(0.5)
    try:
        os.close(fd)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    os.unlink(rcfile_path)
    if rc_dir != OUT:
        try:
            os.rmdir(rc_dir)
        except OSError:
            pass
    return bytes(out)


ZSH_SCENARIOS = {
    # THE scenario this batch exists for. zsh's default PROMPT_EOL_MARK marks output that doesn't
    # end in a newline with a reverse-video placeholder, then pads to the terminal width with
    # spaces, then CRs back to redraw the prompt on the same row — a shape bash simply does not
    # have, since bash never marks a missing trailing newline at all. `printf` with no `\n` is the
    # direct way to trigger it without depending on any particular command's own behaviour.
    "zsh_eol_mark": [b"printf 'no newline here'" + CR],

    # The themed prompt (SGR colour + a multi-byte glyph) through zle's OWN redraw path rather than
    # readline's — same motivation as bash's prompt_echo, different line editor. If the byte-indexed
    # vs column-indexed cursor bug C13_TODO #19 was about exists on zle's side too, this is where it
    # would show: the glyph is exactly where the two disagree.
    "zsh_prompt_echo": [b"echo hello" + CR],

    # zle line editing: emacs-style keys (bindkey -e above) exercised the same way edit_in_place
    # does for readline — home, forward-word, kill-to-end, accept. zsh's zle redraws differently
    # from readline internally even when the KEYS are the same, so this is not a redundant capture.
    "zsh_edit_in_place": [b"echo alpha beta gamma", b"\x01", b"\x05", b"\x17", b"\x17", CR],

    # The right-margin case for zle specifically: readline and zle defer-wrap differently in some
    # implementations' redraw math, and wrap_long_line only ever exercised readline's.
    "zsh_wrap_long_line": [b"echo " + b"x" * 90 + CR],
}


def main():
    os.makedirs(OUT, exist_ok=True)
    wanted = sys.argv[1:] or sorted(SCENARIOS) + sorted(SYNTHETIC) + sorted(ZSH_SCENARIOS)
    for name in wanted:
        if name in SYNTHETIC:
            data = SYNTHETIC[name]
        elif name in SCENARIOS:
            rcfile = os.path.join(OUT, ".bashrc.tmp")
            data = capture(SCENARIOS[name],
                            ["/usr/bin/bash", "--noprofile", "--rcfile", rcfile, "-i"],
                            BASHRC, rcfile)
        elif name in ZSH_SCENARIOS:
            # zsh has no --rcfile: it reads $ZDOTDIR/.zshrc (or $HOME/.zshrc), so the rc lives in
            # its own throwaway directory under a fixed filename instead of an arbitrary path.
            zdotdir = os.path.join(OUT, ".zdotdir.tmp")
            data = capture(ZSH_SCENARIOS[name], ["/usr/bin/zsh", "-i"], ZSHRC,
                            os.path.join(zdotdir, ".zshrc"), extra_env={"ZDOTDIR": zdotdir})
        else:
            sys.exit("unknown scenario: %s (have: %s)" % (
                name, ", ".join(sorted(set(SCENARIOS) | set(SYNTHETIC) | set(ZSH_SCENARIOS)))))
        path = os.path.join(OUT, name + ".bin")
        with open(path, "wb") as fh:
            fh.write(data)
        print("%-24s %6d bytes -> %s" % (name, len(data), os.path.relpath(path, HERE)))


if __name__ == "__main__":
    main()
