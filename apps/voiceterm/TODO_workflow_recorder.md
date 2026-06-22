# Workflow Recorder — voice-named command sequences

## What the user wants

The user wants to record a sequence of shell commands as a named "mini routine"
and replay it with a single voice command. Concrete example from the request:

> "Take everything from cd to dir, then unzip -ou ../file.zip, then cd build,
>  cmake, make -j, cd -. Save as `unpack-build` mini-routine. Next time I say
>  'run unpack-build' it replays the sequence."

This is essentially a macro-recording system on top of the shell with voice
naming and voice playback.

## Scope

This is a meaningful feature but it's NOT a one-session change. It needs:

1. **Recording UI** — let the user mark "start recording" and "stop recording",
   with visual feedback that recording is happening (toolbar indicator? status
   bar? marker on terminal blocks?).

2. **Recording mechanism** — capture the SUBMITTED commands (each Enter press
   that executed). We already track these via `BlockTracker::submit_command`,
   so the recording layer just snapshots a range of block IDs.

3. **Naming** — at stop-recording, prompt for a name. Use voice via the
   existing voice popup with an "intent: name workflow" path, OR use a
   small text input dialog. Voice is the right primary path given the rest
   of the app's affordances.

4. **Persistence** — save to `~/.config/web2qml/workflows.bin` (bincode-serde,
   matching `archive.db` and `voice_fingerprints.bin` patterns). Each entry:
   ```rust
   struct Workflow {
       name: String,
       commands: Vec<String>,
       cwd_at_start: Option<PathBuf>,  // optional: cd to this before replay
       created_at: u64,                 // unix time
       run_count: u32,
   }
   ```

5. **Voice trigger** — phrase pattern "run <name>" / "play <name>" /
   "execute <name>". Add a new match function `match_workflow_replay(text,
   workflow_names)` that fuzzy-matches name with the same separator-collapsed
   logic the entity resolver uses (so "run unpack build" matches workflow
   named "unpack-build").

6. **Replay mechanism** — send each command as a separate line to PTY, with
   delays between commands so they don't interleave. Actually no — better
   to wait for each command to complete (block_tracker reports completion)
   before sending the next. Otherwise `cmake` would start before `cd build`
   finishes parsing.

7. **Replay safety** — workflows that contain `rm`, `dd`, `:>`, `mkfs`,
   etc. are dangerous to replay blindly. Either refuse to record those, or
   prompt for explicit confirm on replay.

8. **List/edit/delete UI** — show recorded workflows in a list view. User
   should be able to delete workflows, rename them, see what commands they
   contain. Probably a modal popup accessible from the command palette
   (`☰` button).

9. **Replay in current cwd vs original cwd** — workflows like "unpack-build"
   are MEANT to be run in whatever directory the user is currently in. But
   workflows that started with `cd ~/Code/proj` are tied to that path.
   Heuristic: if the workflow's first command was a `cd` to an absolute
   path, replay starts there. Otherwise replay starts in current cwd.

## Estimated size

~300-400 lines Rust + ~150 lines QML. Plus testing on real workflows the
user records. Single-session scope is recording + naming + simple replay.
List/edit UI and safety prompts are follow-ups.

## Architectural notes

- **Don't reuse block_tracker for storage.** Workflows are user-curated, blocks
  are session-ephemeral. Separate file, separate type.

- **Voice naming should accept phrase-shaped names.** "unpack and build" is a
  fine name even though the user may have typed it with a hyphen. Store
  the raw spoken form AND a sanitized command-friendly form (kebab-case).

- **Replay should be observable.** User watches each command run with normal
  PTY output. Don't suppress it. Keeps the user in control of "stop if this
  goes wrong".

- **Pause/abort during replay.** Ctrl-C during replay should abort the rest
  of the sequence, not just the current command. Track replay state on
  Backend.

- **Conflict with shell aliases/functions.** If user has a shell alias
  `unpack-build` defined, voice "run unpack-build" should disambiguate
  between the workflow and the alias. Workflow takes precedence by
  default; show the conflict in the candidate list.

## Open questions

- Should workflows have parameters? "run unpack-build for ../foo.zip" where
  the zip path is a parameter? Yes, eventually — but the v1 should be
  parameterless to keep complexity bounded.

- Should workflows be exportable/shareable? Probably not in v1. If the user
  wants to share, they can read the bincode file.

- Should we record the WORKING directory at each step? Helps reproduce when
  a workflow has `cd dir1 && do_thing && cd dir2`. v1 just records commands
  and assumes a linear cwd progression.

- Should we record OUTPUT to verify the workflow worked? No — too brittle.
  Output changes between runs (timestamps, build numbers, etc.). Trust
  exit codes instead. v2 could check exit code of each step and abort
  the workflow if any step fails.

## What to build first (next session)

1. `Workflow` struct + bincode persistence (50 lines)
2. `WorkflowRegistry` with load/save/add/remove (60 lines)
3. Recording start/stop tracked on Backend, capturing block command strings
   between marks (40 lines in Backend)
4. QML toolbar button "Record" → toggles recording state with visual
   indicator (40 lines QML)
5. On stop, voice popup opens with prompt "Name this workflow" — special
   intent kind `name_workflow` that takes the recognized text as the name
   and calls Backend::save_workflow (60 lines)
6. `match_workflow_replay` voice helper (40 lines)
7. Wire replay: when match_workflow_replay returns Some, sequentially send
   each command to PTY with completion-await between them (80 lines)

That's ~370 lines. One session.

---

## v0.1.577 PROGRESS — pure core BUILT + tested (steps 1, 2, 6, + safety)

src/workflow.rs (~590 lines, 12 tests, all passing via rustc --test).
Registered in main.rs + the pure-check set (suite now 470).

### DONE (pure, verified)
- `Workflow { name, slug, commands, cwd_at_start, created_at, run_count }`
  + `Workflow::new(spoken_name, commands, created_at)` which derives the
  kebab slug and anchors cwd ONLY on an absolute `cd /abs/path` first
  command (relative cd / `cd -` / `cd ~` don't anchor — routine runs in
  current cwd). [build step 1]
- `slugify` (spoken → kebab), tested incl. punctuation/underscores.
- `WorkflowRegistry`: new/len/all/add/remove/get/mark_run. add() REPLACES
  on same slug (re-record updates) and PRESERVES the old run_count.
  [build step 2]
- Hand-rolled JSON persistence (to_json/from_json) — NO serde, matches
  archive.rs convention. Round-trips commands incl. embedded quotes;
  lenient parse skips malformed entries instead of losing the file.
- `match_workflow_replay(text, slugs)`: "run/play/execute/replay/do
  <name>" → slug, exact + hyphenless-collapse match. PRECISION over
  recall — unknown names return None (never replay the wrong sequence).
  [build step 6]
- `dangerous_commands(commands)`: flags rm -r / dd / mkfs / shred /
  sudo / force-push / device-truncate for replay confirmation. [step 7]

### STILL PENDING (live surface — needs Backend + QML + running build)
- Recording: on Backend, mark start/stop; capture BlockTracker submitted
  command strings between marks (build step 3). The commands feed
  Workflow::new.
- Persistence IO: Backend reads/writes ~/.config/web2qml/workflows.json
  using WorkflowRegistry::to_json/from_json (the file IO is the only
  non-pure part; the format is done).
- Voice naming: on stop, voice popup with a `name_workflow` intent that
  passes recognized text to Workflow::new (build step 5).
- QML record button + indicator (build step 4).
- Replay: when match_workflow_replay returns Some, cd to cwd_at_start if
  set, then send each command to PTY awaiting block completion between
  them; if dangerous_commands non-empty, confirm first (build step 7).
- Conflict with shell aliases: workflow takes precedence, show in
  candidate list.

The risky/fiddly pure logic (slug derivation, cwd anchoring, JSON
round-trip with quoting, precise replay matching, danger detection) is
done and locked behind tests. What remains is wiring it to BlockTracker,
the PTY replay loop, and QML — all needing the running build.

---

## v0.1.590 — BUILD FIX: stateful API implemented (live wiring had been half-done)

User's build broke: `mod workflow` declared twice + 9 workflow:: calls
in main.rs/term_blocks.rs referenced functions that didn't exist. A
compacted-out turn had written the call sites (the live wiring) but not
the implementation, and since the full crate can't compile in-sandbox
the break wasn't caught until the user built locally.

FIXED:
- Removed the duplicate `mod workflow;` (main.rs had two).
- Implemented the stateful layer in workflow.rs on top of the pure core:
  start_recording/record_command/stop_recording/is_recording (in-progress
  buffer), start_playback/next_playback_command/cancel_playback (queue),
  create_from_commands (build+persist, Err("exists") on collision),
  list_workflows. Persistence: ~/.config/web2qml/workflows.json via the
  registry's to_json/from_json. Process-global state via OnceLock<Mutex>
  (mirrors corpus_recorder.rs).

Verified as far as the sandbox allows: module compiles, all 9 functions
match their call-site signatures (Option<String>/Result<String,String>/
bool), types align, no orphan refs, pure suite green (563). NOT verified:
full main.rs build (Qt/FFI/edition2024 block cargo here) — needs the
user's build to confirm no OTHER wiring errors remain.

Still genuinely pending (UI/PTY, needs running build): QML record
button, the PTY replay loop that pumps next_playback_command() with
completion-await between commands, and the danger-confirm gate before
replaying dangerous_commands().
