# Filetype → command associations from history (v0.1.686)

## Why (user)
"I was trying to `ffplay cymatics/tribal.mp3` ... the command never caught on
... maybe we should make calls to zsh history to observe filetype
associations." The decision cache hadn't seen the exact phrase, so the voiced
"play tribal.mp3" didn't resolve to ffplay.

## Core (src/filetype_assoc.rs, 8 tests)
build(history_lines) → AssocTable mapping ext → commands ranked by frequency.
- Strips zsh extended-history meta (`: <ts>:<dur>;cmd`).
- Skips generic builtins (cd/ls/rm/cp/cat/...) that don't imply a "handler".
- Ignores flags (-x) and URLs; rejects dotfiles (.bashrc) as extensions.
- Handles sudo/env-var/path prefixes; first pipeline segment only.
best_for(ext) → the most-used command (e.g. "mp3" → "ffplay").
qt_method filetype_command(ext): lazy-loads $HISTFILE / ~/.zsh_history /
~/.bash_history on first call, caches the table.

## Status: WIRED v0.1.687
WIRED: voice_parse_candidates now, when the utterance has an open verb
(is_open_verb: play/open/watch/view/...) AND a token that file_open_command
resolves to a learned handler, surfaces "<handler> <file>" (e.g. "ffplay
tribal.mp3") as a 0.88-score TerminalCommand candidate — beating weak cache
hits. Uses a process-wide OnceLock-cached AssocTable (filetype_assoc_cached)
built once from $HISTFILE/~/.zsh_history. file_open_command quotes paths with
spaces. 11 tests. Needs the build to confirm the candidate appears for "play
X.mp3" end-to-end, but the resolution logic is unit-tested.
