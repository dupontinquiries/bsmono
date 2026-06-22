# Terminal tabs + folder-picker `cd` flow

Status: **DESIGN ONLY (filed post-v0.1.519).** F7: running `cd` via
the folder picker while helix is open injects `cd` into helix's buffer
(wrong — it hijacks the running program). Should offer a new term tab
instead, with a full tab interaction model.

## The bug

The folder picker writes `cd <dir>\n` to the PTY regardless of what's
running. If a full-screen program (helix, less, htop) owns the
terminal, that text goes into ITS buffer, not a shell. We already KNOW
when a foreground program is running (foreground_command,
foreground_is_pager from v0.1.500, alt-screen detection) — so the
picker can detect "not at a shell prompt" and refuse to inject.

## Desired behaviour (user)

1. Folder-picker `cd` while a program is running → do NOT inject.
   Instead PROMPT: "Open a new terminal tab in <dir>?" with
   confirm / deny, answerable by BUTTON or VOICE.
2. On confirm → spawn a new term tab cd'd to that dir.
3. Term tabs openable by pressing the term tab (bottom-bar) button:
   - **mouse**: click = sticky popup of tabs;
   - **touchscreen**: tap = cycle to next tab;
   - **tap-and-hold** (both) → the GestureHost/SlidePopup popup
     listing tabs, each with an NLP SUMMARY of its buffer in a
     condensed list.

## Design

### Detection (pure-ish, reuses existing)
"Safe to inject cd" = at a shell prompt = NOT(foreground program is a
full-screen/alt-screen app) AND last line looks like a prompt. The
pieces exist: pty_term::foreground_command / foreground_is_pager /
alt-screen flag. A small predicate `pty.accepts_shell_input()` wraps
them. Pure logic over those inputs → testable with synthetic states.

### The confirm prompt (reuses F9 voice-hinted buttons)
A confirm/deny dialog that takes BOTH a tap and a voice hint — this is
exactly docs/design/voice_hinted_buttons.md (F9). The "yes/open tab"
and "no" buttons animate when a matching voice phrase is detected.
Shared primitive.

### Multi-tab model (reuses bottom_bar + GestureHost/SlidePopup)
Term tabs are already a concept; this needs:
  - a tab LIST model (id, cwd, foreground program, buffer summary);
  - the bottom-bar Tabs button (bottom_bar.rs BarButtonKind::Tabs —
    already designed!) drives it;
  - mouse-click = sticky SlidePopup; touch-tap = cycle; tap-hold =
    SlidePopup with summaries. GestureHost already distinguishes
    tap vs hold-drag, so the popup path is the existing primitive.

### NLP buffer summaries (new small pure piece)
For each tab, a condensed one-line summary of its buffer (what's
running / last command / file open). A pure summarizer over the
buffer text — could reuse log_summary / the block tracker's command
history. Testable: buffer text → short label.

## What's pure/testable vs live
- TESTABLE: accepts_shell_input() predicate, the tab-list model,
  the buffer→summary function, tap-vs-cycle-vs-hold decision logic.
- LIVE: the actual new-tab spawn, the popup rendering, the picker
  refusing injection, voice-hint animation — needs the running build.

## Reuse / alignment
- bottom_bar.rs BarButtonKind::Tabs + options_for + resolve are
  ALREADY the tab-fan-out router (built v0.1.507). This is its first
  real consumer.
- GestureHost (tap vs hold-drag) + SlidePopup (the fan-out list) —
  the popup is the existing terminal pipe/brace primitive.
- voice_hinted_buttons (F9) — the confirm/deny dialog.
- foreground_is_pager / foreground_command (v0.1.500) — the detection.
This issue is the convergence point of several built-but-unwired
primitives (bottom bar, gesture host, voice hints) — wiring it
surfaces three things at once.

## Open questions
- Where do term tabs live structurally today — is there already a
  multi-PTY container, or is it single-PTY (pty: Option<PtyTerminal>)?
  (Determines if "new tab" needs a Vec<PtyTerminal> refactor first —
  likely the biggest real work here.)
- Touch cycle order: MRU or creation order?
