# TODO: auto-named sessions + mental map for short-burst memory (R2)

## Priority: SOLID (clear value; one part speculative)

## The idea (user)
Auto-name sessions from ACCURATE real signal, plus a MENTAL-MAPPING layer
to aid short-term recall when the user bounces away and back (reboot /
interruption / window-switch). Serves the context-loss + re-entry theme.

## Two parts, different confidence

### Part 1 — auto-naming (do first; clearly good, pure, testable)
Generate a session name from real signal:
  - git repo + branch, the files touched, the dominant command/verb, the
    host (local/ssh). e.g. "asahi-linux · dcp-reversing · python tracer".
Pure function: (signals) → name. Testable in-sandbox. Pairs with recall +
R5 (cluster label could supply/improve the name).

### Part 2 — mental map (more speculative; start cheap)
A memory hook so the user can re-grab a session fast. Options, cheapest
first:
  - **docker-style handle**: memorable adjective-noun (e.g. "amber-mailbox")
    — pure, tiny, testable name generator. CHEAP START.
  - **geometric/spatial**: position sessions on a plane (reuse cards-canvas
    geometry) so location = memory. More work, more speculative.
  - learned patterns from R5 clusters.
CAUTION (round-2 principle): the mental map must be a genuine MEMORY AID,
not decoration — if it doesn't measurably help re-entry, it's clutter.
Start with the docker-style handle (cheap, removable) and only add spatial
if it earns it.

## Wiring
Session metadata + recall; re-entry surfacing uses the assist-bubble
primitive ("here's where you were: <name>, last cmd, last diff").

## Status: not started. Part 1 first (pure auto-namer). Part 2 cheap-first.
EOF

## DONE v0.1.658 — pure core (session_auto.rs, 10 tests, validated on Hector's stream)
Built the context-shift detector + proposals + auto-naming (R2 Part 1 +
the "this session needs a new tab for this python script with helix"
insight):
- Activity{kind,target,cwd} stream; ActivityKind Cd/Run/EditorOpen/NewFile/
  Build/Test/Reboot; lang_of() ext→language.
- segment(acts) → Vec<Segment> with boundaries at repo-change + reboot,
  each auto-named (repo · dominant-file/lang).
- propose(open_files, dominant_lang, repo, next, editor) → Proposal:
  NewTabForFile (start a 2nd/dshifted file → tab+editor), NewSessionForRepo
  (cd to a different repo), ReentryAfterReboot (restore context).
- name_segment(acts) → "repo · file" / "repo · lang".

### Validated on the REAL corpus
Derived a 103-event activity stream from Hector's transcript (ordered
markers; 6 reboots). The segmenter split his ONE linear terminal into 7
named sessions (at the reboots) and would surface 6 ReentryAfterReboot
bubbles + a NewTabForFile when he switched scripts — directly attacking his
#1 pain (context loss after reboot) and giving the per-task structure he
can't spare keystrokes for.

### STILL TO WIRE (build-gated)
1. Feed real pty/editor events into Activity (cwd from prompt, cmd from
   block_tracker, editor-open from the open_in_editor path, reboot from a
   disconnect/boot signal).
2. Surface Proposal via the assist-bubble primitive (TODO_assist_bubble.md)
   — transient, dismissable, NEVER auto-applied.
3. On accept: NewTabForFile → open_in_editor in a new pane; NewSessionForRepo
   → workspaces::new; ReentryAfterReboot → recall + re-present open files.
4. Tune segmentation (tiny inter-reboot gaps merged; min-session size).

## WIRED v0.1.720 — backend qt_method live
- session_autoname() qt_method: derives an Activity stream from the command-
  block history (_activities_from_blocks classifies each command's leading verb
  → Cd/EditorOpen/NewFile/Build/Test/Run) and returns name_segment(acts).
  Verified on a realistic stream: "cd asahi/dcp; hx tracer.py; python tracer.py
  …" → "me · tracer.py". Empty until ≥2 activities.
- REMAINING (QML): surface the name on the Session window / assist-bubble
  re-entry ("here's where you were: <name>"). Backend is ready; the QML
  surfacing is the next step (held — low risk but unverifiable feel).

## WIRED v0.1.721 — re-entry surfacing + cluster-blended name
- session_autoname now blends the dominant command-cluster label (R5) into the
  name when there's a clear plurality task group: "me · tracer.py" → "me ·
  tracer.py · python". Verified on a python-heavy stream.
- session_reentry_assist() qt_method surfaces "Where you were: <name>" + a
  "↻ last: <cmd>" option through the shared assist-bubble channel (scheduler
  rate-limited). QML calls it on window foreground after a ≥30s absence (a brief
  focus blip won't trigger it). The R2 Part-1 + assist-bubble integration the
  TODO earmarked.

## Task clusters surfaced (v0.1.723)
The sessions page (web2qml://sessions) now appends a "Task clusters" section:
each discovered command group with its label, size, and example commands
(_cluster_markdown). The R5 work is now visible end-to-end.

## DONE in the egui port — the name is on screen, and one naming bug fixed

The egui frontend never referenced `session_auto` at all: `grep` found
nothing, so the segmenter, the namer and the proposals were carried
across and none of their output was.

- `session_name.rs` ports the Qt `_activities_from_blocks` derivation out
  of its QObject method into a free function over `(command, cwd)` pairs,
  where it can be tested — it never had a test. Improved on the way:
  `sudo make install` and `RUST_LOG=debug cargo test` are a Build (the Qt
  version reads word 0 flat and calls both a Run), a path-qualified
  editor is still that editor, and an editor's file is the first non-flag
  argument. Floor of two activities: `cd ~` is not a session about `~`.
- The name is appended to the LOCATOR line, beside where you are:
  "1/2 main · dcp · tracer.py". Recomputed when a command finishes, not
  every frame — the derivation walks every block and the locator draws
  constantly.
- BUG FIXED in the shared `name_segment`: it took the repo from the FIRST
  activity's cwd, and the first activity in a session is the `cd` INTO
  the work, run from wherever you were. On this module's own quoted
  stream (`cd asahi/dcp; hx tracer.py; …`) that named the session after
  the directory it left — the doc's own example output, "me · tracer.py",
  shows the same artifact. It uses the DOMINANT cwd now, ties to the
  earlier one so the name is stable.

STILL OPEN: step 1's "cwd from prompt". This frontend's block tracker only
learns a cwd from `set_cwd`, so the repo half of the name is whatever was
last set rather than where the command actually ran. The file half works
regardless. Steps 2–4 (proposals via the assist bubble, accept actions,
segmentation tuning) are untouched.

### Aside, found while walking this under Xvfb
A scripted `touch tracer.py` followed by another command produced ONE
merged block. Not a tracker bug: `touch` is on `cmd_risk::MUTATING`, so
the Enter was withheld for the confirm dialog and the automation typed
straight past it. The gate working as designed — recorded here because it
looks exactly like a lost keystroke.
