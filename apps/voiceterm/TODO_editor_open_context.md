# TODO: editor-open with advanced context (R1) — vim + helix, SSH + deep path

## Priority: HIGH (most concrete + highest value of the round-2 ideas)

## The idea
Open an item directly into the user's editor WITH context already resolved:
a deep file path (via our extractors), a line number, and crucially the
right HOST (local or an SSH machine). Example utterance:
  "open the dcp tracer on the m1n1 box at line 400"
→ resolves: editor (hx/vim) + host (m1n1 via ssh) + fuzzy path (dcp tracer)
  + line (400). Both vim AND helix (he uses vim; helix matters for the next
  wave of devs).

## What already exists (so this isn't from scratch)
- path_fuzzy::fuzzy_match(voiced, limit) — voiced phrase → ranked real
  paths. (src/path_fuzzy.rs)
- entity_extract — pulls entities (paths, etc.) from text.
- editor_voice — editor detection + motion grammar (detect_editor: hx/vim).

## What's NEW to build
1. **SSH-target resolver (pure, testable first slice).** A small module:
   known hosts (from ~/.ssh/config + recent sessions) + fuzzy-match a
   voiced host name → host. e.g. "the m1n1 box" → "m1n1". Pure string/fuzzy
   work — build + test in-sandbox.
2. **An open spec + builder.** Combine (host?, path, line?, editor) → the
   command: local `$EDITOR +LINE path` or `ssh host -t '$EDITOR +LINE
   path'`. Pure to build the argv; spawn is the wiring.
3. **Voice intent.** "open <thing> [on <host>] [at line N]" → OpenInEditor
   spec. Lead-in + slot parsing (host slot, line slot), mirror the recall
   query-extraction pattern.
4. **Wiring.** Resolve path via path_fuzzy, host via the resolver, spawn in
   a pane (zellij new-pane or the terminal). Compile-checkable now.

## Suggested first slice
Pure SSH-target resolver + the argv builder (host/path/line/editor → argv)
with tests — proves the hardest pure part. Then the voice intent, then the
spawn wiring. Each compile-verifiable.

## Status: not started. Strongest candidate to BUILD.
EOF

## DONE v0.1.653 (pure core, compile-verified)
src/editor_open.rs (9 tests, in harness, compile gate green):
- Editor { Vim, Helix } + from_voiced (vim/nvim/neovim/vi, helix/hx) + bin().
- parse_ssh_hosts(config_text) → host aliases (skips */?/! patterns + comments,
  multi-alias Host lines, case-insensitive keyword).
- resolve_host(voiced, known) → fuzzy best host (strips fillers "the/box/
  machine/on"; exact > token > substring-overlap; ≥0.5 bar).
- OpenSpec { editor, path, line, host } + build_open_argv → local
  `<ed> +LINE -- path` or remote `ssh -t host <ed> +LINE -- path` (-- flag
  guard; -t for TUI over ssh).

## STILL TO WIRE (build-gated, next slices)
1. ~/.ssh/config read + recent-hosts merge → known-hosts list (I/O around
   the pure parser).
2. Voice intent: "open <thing> [on <host>] [at line N] [in vim|helix]" →
   slot-parse (host/line/editor slots) → resolve path via path_fuzzy,
   host via resolve_host → OpenSpec. Mirror the recall query-extraction.
3. Spawn: build_open_argv → run in a pane (zellij new-pane / terminal).
4. (optional) default editor from $EDITOR / detect_editor when unspecified.

## WIRED v0.1.654 (intent + backend, compile-verified end-to-end)
- editor_open::parse_open_request(phrase) → OpenRequest{thing, host, line,
  editor} — pure slot parser (verb strip; editor "in vim/helix"; line
  "at/on line N"; host "on <X>"; "on line" not mistaken for host). 6 tests.
- voice intent OpenInEditor(String): triggers on edit-verb / explicit
  editor / line / host markers so plain "open <site>" stays navigation
  (tested both ways). Compile-verified via gate.
- backend open_in_editor qt_method (3 sites, qt_struct 445): slot-parse →
  fuzzy path via path_index (falls back to literal) → host via
  ~/.ssh/config (parse_ssh_hosts + resolve_host) → build_open_argv →
  shell-quote → pty_write onto the input line (NO auto-run; user reviews).
- QML dispatch: switch to terminal, focus, call backend.

## STILL build-gated (your run to confirm behavior)
- The typed command appears on the terminal input line correctly + the
  fuzzy path resolves to what you meant (path_index quality, on-device).
- Default editor when unspecified is currently vim; could honor $EDITOR /
  detect_editor. Small follow-up.
- Optional: spawn in a NEW zellij pane instead of the current line.
