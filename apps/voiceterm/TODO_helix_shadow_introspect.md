# Helix voice control + shadow-instance introspection

User request (v0.1.576): two parts, near-term DONE + research idea filed.

## Part A — voice → helix pickers (DONE v0.1.576)
"show project files" → helix `<space>f` file picker, plus global search
(`<space>/`), buffers (`<space>b`), symbols (`<space>s`), command
palette (`<space>?`), file explorer (`<space>e`). In editor_voice.rs
translate_helix, top exact-match block (before the find/search prefix
handlers so "find files" → picker, not /-search). Tested:
helix_space_pickers, 16/16 editor_voice tests pass.

These just send the leader chord as PTY keystrokes — the same mechanism
as every other editor_voice command. No shadow instance needed.

## Part B — shadow-helix + process watcher (RESEARCH, not built)
User's framing: "a duplicate helix rolling back with some debugger and
file cloning process watcher to analyze what happens when we tap into
helix file picker or fuzzy search or run commands to get more context."

Goal as I understand it: run a SECOND, hidden helix instance in
lockstep, instrument it (debugger / strace / fanotify), and observe what
it does when a picker/search/command fires — so web2qml can learn the
file set, the search index, the working context, WITHOUT scraping the
visible terminal. Effectively: use a shadow editor as an oracle for
"what files/symbols exist here and what did that command touch."

### Why this is its own research arc (not a turn-and-ship)
- **Process introspection is heavy + platform-specific.** Options:
  - strace/ltrace the shadow helix → parse openat()/read() syscalls to
    learn which files it touched. Linux-only, needs ptrace (which our
    own hardening DISABLES — see [security] no-ptrace=on; we'd have to
    carve an exception for a child we spawn).
  - fanotify/inotify on the project dir → watch what helix opens.
    Lighter than ptrace, no syscall parse, but only sees file opens,
    not in-memory picker state.
  - gdb attach → too heavy, fragile across helix versions.
- **Lockstep replay is hard.** Keeping a shadow helix in the same state
  (same file, same cursor, same picker query) as the visible one means
  mirroring every keystroke to both PTYs and handling divergence. Helix
  has no scripting API, so it's all keystroke puppetry + output scrape.
- **Most of the value may be obtainable cheaper.** What we actually want
  is "the project's file set + symbol index." helix gets that from the
  workspace (a `.git` root walk + LSP). We can get the same DIRECTLY:
  - file set: walk the project root honoring .gitignore (we already
    have skip_dirs / project_markers infra).
  - symbols: query the same LSP helix uses, or ctags, ourselves.
  So before building shadow-helix, PROTOTYPE the direct approach and see
  if it covers the use cases ("insert a filename", "jump to symbol").

### Recommended sequencing
1. (DONE) Part A: voice fires the real helix pickers.
2. Direct project-introspection prototype: file-set walk + optional LSP/
   ctags symbol list, exposed to voice ("insert filename X", "open the
   file with Y"). Pure-ish, testable. Likely covers 80% of the value.
3. ONLY IF that's insufficient: the shadow-helix + fanotify watcher, as
   a focused arc with the ptrace/fanotify decision made up front and the
   security-hardening exception scoped to the spawned child.

The shadow approach is genuinely interesting for "learn what an opaque
tool does," but it's a large, platform-specific, security-sensitive
build. File it; prototype the cheap direct path first.

---

## v0.1.580 — the cheap DIRECT path advanced: file RANKING now exists

Recommended sequencing step 2 (prototype direct project-introspection
before shadow-helix) moved forward. The gap was: candidates_for_voice
did glob→filter but returned an UNRANKED recall-biased list (up to 50
files). resolve_reference (entity_extract) already RANKS a spoken phrase
against entities with great filename-collapse logic — but nothing
adapted bare file paths into the Entity form it needs.

BUILT (pure, tested) in completion.rs:
- files_to_entities(paths) → Vec<Entity{File}> with display=basename,
  aliases=[basename, stem, full path] so "main" matches main.rs.
- rank_paths_for_voice(paths, voice_text, top_n) → ranked (path, score),
  composing files_to_entities + resolve_reference.
- resolve_file_for_voice(cwd, voice_text, top_n) → the full pipeline
  (glob → prefilter → rank). IO isolated in glob_cwd_files; the rest is
  pure and tested (rank_paths_for_voice has direct tests: "open the
  readme" → README.md first; "open main" → main.rs via stem).

This reuses the existing matcher (no duplication of resolve_reference).
Tested by staging completion + entity_extract + a pty_term stub: 16/16.

STILL PENDING (live wiring, needs running build):
- A voice intent ("open the <file>", "edit <file>") that, on the
  terminal/editor surface, calls resolve_file_for_voice(shell_cwd, text)
  and acts on the top result: in a shell → type the path at cursor; in
  helix → `:open <path><ret>` (or feed the picker). Backend + voice_intent
  wiring; can't test here.
- Confidence gating: if top score is low or the top two are close, show
  a picker instead of auto-acting (the resolve_reference score + reason
  are already returned to support this).

So the direct path now RESOLVES + RANKS files purely. If this covers the
real use cases once wired, the shadow-helix approach (Part B) may not be
needed at all — re-evaluate after dogfooding the direct path.
