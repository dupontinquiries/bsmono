# Deferred ideas from v0.1.67 testing

User raised many ideas during v0.1.67 testing. The genuine bugs and
small fixes shipped in v0.1.68. The bigger ideas captured here for
future sessions, in roughly the order I'd suggest tackling them.

## 1. Touchscreen filepicker for `cd`

User idea: when the user voices "cd" or starts a path, show a
filepicker with macOS-style horizontal drawers — each subdirectory
becomes a column to the right. User taps to navigate, hits checkmark
when done.

Why this matters: voice for paths is the worst case for whisper
because path components rarely match a dictionary. "kwin" gets
transcribed as "k win" or "queue when". A visual picker is
zero-error.

Power-user binding: double-tap on the voice mic icon (or a settings
toggle) opens the picker for the next utterance instead of starting
recording. Or a voice keyword like "browse to" / "pick from".

Implementation sketch:
- New QML component: HorizontalDrawerPicker, takes initial path
- Backend method: `list_dir(path) -> [{name, is_dir, mtime}]`
- Each drawer: a vertical ListView of entries, tap to push next
  drawer or select file
- "Done" button at top-right of current drawer commits as `cd <path>`
- Subdirs lazy-load (only fetch when drawer opens)

Estimated size: ~300 lines QML + ~50 lines Rust. One session.

## 2. Sub-word audio matching — composing terms from word fragments

User question: "is there a way we can take audio clips and match
portions of longer words to letters or small system commands like if
we have `mkdir` or `tokei` being able to be constructed from words
like 'make' 'direction' 'today' etc."

The technique: phoneme decomposition. Whisper internally maps audio
to subword tokens. We could:
- Run whisper at lower confidence threshold to get a phoneme-ish
  stream rather than coherent words
- Build a phoneme-similarity index over known commands (mkdir =
  /m k d ɪ ɹ/, tokei = /t oʊ k eɪ/)
- For each utterance, compute the best-match command from the index

This is partway between whisper and our current MFCC fingerprinting.
The fingerprinting captures whole-word audio signatures; this would
capture sub-word audio signatures so combinations are recognizable.

Substantial work — probably 1000+ lines + a phoneme model dep. Defer
until the fingerprint DB is at scale and we have data to validate
against.

## 3. Live monitoring tools (`top`, `htop`) — buffer per-update for context

User report: voicing "kill jami" while `top` is running, with `jami`
visible in top's output, didn't get matched as the entity. The
context wasn't there because top's output is alt-screen and we don't
buffer it.

Possible fix: when we detect `top` / `htop` / `iotop` / similar in
the foreground command, periodically (every 1-2s) snapshot the
visible alt-screen content into a separate "monitoring buffer". Use
THAT as entity context for kill / stop / pause commands.

Detection: foreground command name matches a known monitoring tool.
Snapshot: `pty.grid.lock().get_visible_text()`.

Risks:
- Snapshotting alt-screen contents is invasive (we don't normally
  read alt-screen state from userland)
- Privacy: alt-screen might contain sensitive info (passwords in
  htop's command column for misconfigured services). Need opt-in
  setting.

Estimated size: ~200 lines, and careful UX work for the consent
dialog. One session.

## 4. Background AI for language fix corpus → DB

User idea: "we need to be looking for more of these language easy
fixes by running some actually usable AI in the background and just
saving the results to our databases"

Concrete: every time the user CONFIRMS or REJECTS a candidate, log
the (utterance, candidate, decision) tuple. Periodically (when idle
+ on AC power) run an LLM over the rejected ones with a prompt like:
"the user said X but this command was wrong, what should the
recognized text have been?" The AI's suggestion gets stored as a
fingerprint-label correction and folded into the phonetic match
scoring.

Architecture: ties into the cancellable async_jobs infrastructure
already shipped + the planned background mic polling. Same idle-time
batch-job pattern.

Estimated size: ~400 lines. Two sessions. Depends on fingerprint DB
work being further along.

## 5. helix scroll still not working in v0.1.66

User confirmed: scrolling in helix STILL doesn't work despite our
PgUp/PgDn forwarding. Likely cause: helix uses mouse-mode reporting
(SGR 1006) and ignores our PgUp bytes when its mouse mode is active.
Need to actually send DEC mouse wheel events.

Implementation:
- Detect when PTY has set mouse mode 1006 (looks for `\x1B[?1006h`
  in the terminal output stream)
- When in alt-screen AND mouse mode is on, format wheel events as
  `\x1B[<64;col;row;M` (button 64 = wheel up) or `\x1B[<65;col;row;M`
  (wheel down)
- Convert click pixel coords to terminal cell coords using charW/charH

Estimated size: ~150 lines. Half session.

## 6. Cursor highlighting next-line bug in helix

User: "when the cursor is on either of the last two characters in a
line in helix the entire next line is highlighted that might be a
hint for what is causing some issues"

Diagnosis hint accepted. Probably a wrap detection bug in our PTY
parser — we may be treating an end-of-line position as the start of
the next line, which means our cursor-rectangle highlight (and any
other position-derived overlay) anchors to the wrong row.

Captured in TODO_terminal_layout.md already, but this user-reported
clue narrows the suspect to wrap-position handling specifically.

## 7. `tree depth 3 did not exit man pages before running`

When the user is inside `man` (alt-screen pager) and voices a new
command like `tree depth 3`, we should send `q` to exit the pager
first, then send the new command, NOT just send the command into
the pager (where it does nothing useful).

Detection: foreground command is `man`/`less`/`more` AND user voiced
something that's clearly a new command (not a pager keybinding).

Implementation: in voice_parse_intent's terminal path, when context
shows we're in a pager, prepend `q` + small sleep + new command. Or
detect specifically and do it as a two-step pty_paste sequence.

Estimated size: ~80 lines. Half session.

## Suggested order

If I were prioritizing for impact:
1. Pager exit before commands (#7) — small, frequently encountered
2. Touchscreen filepicker for cd (#1) — high impact for voice ergonomics
3. Background AI corpus building (#4) — feeds the long-term DB
4. Mouse mode reporting for helix scroll (#5) — fixes a current daily annoyance
5. Live monitoring buffer (#3) — niche but cool
6. Sub-word audio matching (#2) — research project, tackle when DB is rich

The cursor-next-line bug (#6) folds into the cursor alignment work
already in TODO_terminal_layout.md.

## Update v0.1.69 — what shipped vs what's still deferred

Shipped: minimal single-drawer file picker triggered by double-tap on
path/folder tokens. clipboard copy on double-tap of url/host/word/
number tokens. Toast confirmation. Token classifier + extraction.

Still deferred:

### Cascading multi-drawer file picker (the original macOS-style idea)

The current drawer replaces its own contents when navigating into a
subdir. The fancier UX you sketched: each subdir opens as a NEW drawer
to the right, like Finder's column view. Lets the user retain
breadcrumbs visually and tap back across columns to retarget.

Implementation sketch:
- Replace single Popup with a horizontal Row of drawer columns
- Each column = ListView model from term_list_dir(its_path)
- Tapping a directory ADDS a new column to the right (and removes
  any to the right of where you tapped)
- Drawer width adaptive: narrower as count grows, with horizontal
  scroll if total > viewport
- "cd here" commits to the path of the rightmost drawer

Estimated size: ~250 lines QML on top of the existing single-drawer
foundation. One session.

### Long-press → paste line as command

User suggested: "double tapping on the line could be for jumping to
that section or pasting". Distinct from the token tap. The line-level
gesture is harder to disambiguate from token tap because a long-press
on a token would naturally feel like "do something with this whole
line".

Proposal: long-press (PressAndHold) on a terminal line → grab the
ENTIRE line text → insert at prompt as a new command. Useful for
re-running something visible in scrollback without typing.

Already partially clashes with current behavior (long-press =
selectAll). Need to decide which wins. Probably: long-press on a
token = current selectAll behavior; long-press on whitespace
between tokens = paste-line behavior.

Estimated size: ~80 lines QML. Half session.

### Pager-aware double-tap

User's idea: "tapping inside a pager program could be for something
else". Right now we no-op the double-tap dispatch in alt-screen
(pager mode). Could surface a small action menu instead — "exit
pager", "search forward", "go to top/bottom", "quit and run new
command". Only in alt-screen.

Estimated size: ~120 lines QML. Half session.
