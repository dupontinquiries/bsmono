# TODO: zellij voice + assisted management

## Vision (from the ask)
Add voice to a power user's zellij arsenal — OPTIONAL, opt-in, costs a
non-zellij user nothing. We don't bet on whether they'll prefer voice over
keys for navigation; the assisted READ side (knowing the pane/tab layout)
is value EVEN for a keys-first user, so lead with the foundation that
serves both.

## Design decision (confirmed vs zellij docs)
Drive zellij via `zellij action <subcmd>` CLI, NOT keystrokes. The CLI is
zellij's entire control surface, it's mode-independent (keystrokes depend
on input mode + custom keybinds), and query actions return JSON. Detection
is via the ZELLIJ env var (set inside a session), NOT /proc/comm — inside
zellij the foreground process is your shell/editor, not `zellij`.

## Done (pure, tested — v0.1.634)
src/zellij_voice.rs:
- `in_zellij_session(env)` — detect via ZELLIJ env var.
- `translate(phrase) -> Option<ZellijAction{argv, confirm, query}>` —
  maps phrases to `zellij action ...` argv. Covers panes (new/split/
  floating/close/focus-dir/fullscreen/toggle-floating/resize-dir), tabs
  (new/next/prev/close/go-to-N), and assisted QUERIES (list-panes --json,
  query-tab-names) flagged query=true. close-pane/close-tab flagged
  confirm=true. Verified action names against zellij docs (list-panes
  --json is the structured read; dump-screen for pane contents).
- `commands()` — the vocabulary list to show when zellij is active.
- 10 tests incl. unknown-phrase→None and bare "pane"→None (no accidental
  destructive fire).

## Wiring (build-gated — needs the running app)
1. **Detection hook**: capture std::env::var("ZELLIJ") for the PTY's
   environment (the spawned shell inherits it). When set, the context is
   "zellij" — surface zellij_voice::commands() the way editor_voice's
   commands surface for helix (main.rs ~10400 voice_cmds_json path), and
   route voice phrases through zellij_voice::translate FIRST when in a
   zellij session (before generic intent parsing), analogous to the
   editor_voice translate hook at main.rs ~12773.
   - NOTE: detection is per-PTY-session env, not the live foreground cmd.
     If the user starts zellij mid-session, ZELLIJ appears in the child
     env but main.rs's captured env may be stale — may need to read it
     from /proc/<pty_pid>/environ or via a `zellij list-sessions` probe.
     Decide during wiring.
2. **Execution**: for a mutation ZellijAction, spawn `zellij <argv>` as a
   subprocess (NOT write to the PTY) — it acts on the session out-of-band.
   For confirm=true, gate behind the existing confirm UX (like editor
   quit-without-save). For query=true, capture stdout and route to the
   assisted layer (3).
3. **Assisted layer (the keys-first-user value)**: parse `list-panes
   --json` / `query-tab-names` output into a small model (pane id, name,
   exited, focused; tab names). Enables: "focus the tests pane" (match
   name → `zellij action ... ` focus by id), "what's running" (read), and
   smart suggestions. This is where voice beats keys — keys can't answer
   "which pane is the build in".
4. **tmux parity (later)**: power-features.md wants a Multiplexer trait so
   QML stays tool-agnostic. zellij_voice is the zellij impl; a tmux impl
   (tmux has an analogous command surface) can follow behind the same
   ZellijAction-like shape. Don't make QML ship tool-specific code.

## Open questions for the user (when they want to push further)
- Auto-detect (surface zellij voice whenever ZELLIJ is set) vs an explicit
  "zellij mode" toggle vs both? (They were unsure — default to
  auto-detect-but-quiet: recognize the commands, don't nag.)
- First slice to flesh out: pane/tab nav (done in mapper) vs layouts/
  sessions vs the assisted reads? (They were unsure — the mapper covers
  nav already; the assisted reads (3) are the differentiated value, good
  candidate for next.)

## Verify on device
Inside a zellij session, say "new pane right" → a pane splits right;
"what tabs" → the assisted layer shows tab names. Confirm a non-zellij
session ignores these phrases (translate returns None, falls through).


## WIRED — v0.1.702
The pure core (translate/in_zellij_session/commands) is now wired:
- qt_methods: zellij_active() (ZELLIJ env detect), zellij_translate(phrase) →
  "cmd\tconfirm\tquery", zellij_commands() → vocabulary wire.
- QML dispatch: in the "freeform" case, when zellij_active(), a phrase is tried
  as a zellij command first; mutations run via pty_write (toast feedback),
  destructive ones (close pane/tab, confirm=true) route through a ConfirmDialog
  (zellijConfirm), and it falls through to web search if not a zellij command.
- REMAINING: the QUERY side (list-panes --json / query-tab-names, query=true)
  is translated but the dispatch currently runs all non-confirm actions via
  pty_write; routing query output to an assisted read ("which pane is the
  build in?") is the next step. The vocabulary overlay (zellij_commands) is
  exposed but not yet shown in the UI.
