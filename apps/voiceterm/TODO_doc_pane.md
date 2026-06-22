# TODO: documentation pane with auto-find

## Vision (user, v0.1.637 session)
"open up extra terminals for documentation with auto find." Voice-spawn an
extra terminal/pane dedicated to docs that AUTOMATICALLY pulls the right
documentation for what you're working on — man page, --help, tldr, or web
— without you specifying the source. A thinking-surface move: docs live
beside your work, hands-free.

## Done (pure, tested — v0.1.638)
src/doc_resolver.rs — the "auto find" brain:
resolve(query, Availability) -> DocSource {BundledManPage, SystemMan,
Tldr, HelpFlag, WebSearch}. Encodes the judgment that makes it smart, not
literal:
- "git rebase" → man git-rebase (subcommands HYPHENATE for man).
- "cargo build" → `cargo build --help` (help-first subcommand tools:
  cargo/kubectl/docker/npm/... — their --help beats a thin man page and is
  version-exact).
- "rg" (real binary) → its man page; bundled page wins when we ship one
  (offline, instant).
- uninstalled-but-tldr-known → Tldr (what is it / how to install).
- opaque phrase → WebSearch (the whole phrase).
DocSource::pane_command() gives the shell command a pane runs (`man
git-rebase`, `cargo build --help`) or None for in-app render. 10 tests.
Availability is supplied by the caller (pure-friendly) from the existing
manpages::get / command_index / PATH index.

## Wiring (build-gated)
1. **Availability hookup** (main.rs): bundled_man = manpages::get(k).
   is_some(); has_tldr = command_index lookup; on_path = the PATH binary
   set (entity cache ~15264). Pure glue.
2. **Context source** — what query to resolve:
   - explicit: voice "docs for git rebase" / "explain rg" → that phrase.
   - implicit (the real magic): the CURRENT command. Use the foreground
     command (pty_term foreground_command) or the last-typed command from
     block_tracker → resolve docs for IT automatically. "open a docs pane"
     with no arg = docs for whatever you're doing.
   - an error/selection: resolve a selected block (ties into the C1
     output→web idea) → usually WebSearch.
3. **Spawn the pane**:
   - In zellij (shell_dialect / ZELLIJ env): zellij_voice-style
     `zellij action new-pane -d right -- <pane_command>`. Reuse the v634
     zellij action plumbing — this is exactly why the CLI-action design
     was right: spawning a doc pane is one `new-pane` action.
   - Not in zellij: render in the app's own doc panel (manpages::to_qml
     for BundledManPage; run pane_command in a transient PTY pane;
     WebSearch → the peer browser tab).
4. **Voice vocabulary**: "docs", "open docs", "explain this", "man page
   for X", "how do I use X" → resolve + spawn.

## Why this is a strong feature
- Differentiated: a generic terminal makes you remember `man git-rebase`
  vs `git rebase --help` vs `tldr`; this picks for you.
- Builds on THREE landed pieces: zellij action plumbing (v634), shell
  detection (v636), and the existing manpages/command_index corpus.
- Hands-free + beside-your-work = the thinking-surface vision.

## Next pure slice — DONE (v0.1.639)
doc_resolver::extract_query(command_line) -> Option<String>: the
implicit-mode brain. Strips env assignments, wrapper commands (sudo/time/
nice/env, incl. their flags + numeric/assignment values), path
qualification, and the pipeline tail; keeps head + subcommand for
subcommand-dispatch tools (git/cargo/kubectl/systemctl/...). 11 tests.
Probe-driven: caught + fixed 3 real bugs (flag-value-as-command,
env-after-wrapper, path-stripped-env false-positive). Documented one
honest limit: a word-valued wrapper flag ("sudo -u bob ls") is ambiguous
to a generic parser — lands on "bob" → resolves to harmless WebSearch; all
common wrapper forms (sudo CMD, nice -n N CMD, env VAR=x CMD) work.

Implicit-mode flow now: extract_query(foreground/last-command) -> resolve
-> spawn. Remaining = the build-gated spawn wiring (availability hookup +
zellij new-pane / app doc panel).
