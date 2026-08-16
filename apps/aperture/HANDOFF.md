# Handoff — aperture session bundle

For whichever Claude picks this up next. Read this before touching code;
it's the map, not the territory — `apps/aperture/DESIGN.md` is the
territory (it's long — ~50KB, one section per commit below, each with its
own "honest ledger" of what's tested vs. compiled-only vs. deliberately not
built) and `TODO.md`'s aperture section has the standing backlog.

## If you fetched a bundle before commit `1fd64e91`, re-fetch

A real bug in my own git workflow (not the code): commit `c89c9f3b`
("the sun follows the clock") edited `shared/scene-kit/src/mesh3d.rs` but
was committed via `cd apps/aperture && git add .`, which never staged that
file — it sat modified-but-uncommitted locally through 10 subsequent
commits. Every bundle regenerated in that window would build fine here
(the local working tree still had the uncommitted file) but would FAIL TO
COMPILE for anyone who actually fetched and merged it, since
`aperture-scene` calls `sun_dir_3d_at`/`sun_clock_at`, which didn't exist
in the committed `mesh3d.rs`. Fixed in `1fd64e91`, found by running a
plain `git status` at the repo root instead of only inside `apps/aperture`.
If in doubt, `git log --stat 1fd64e91` shows exactly what was missing.

Audited afterward: iterated every single commit's actual changed paths
(not commit-message text, which mentions "shared/scene-kit" often enough
to produce false positives on a naive grep) to confirm this was the ONLY
instance of the bug. Exactly three commits genuinely touch
`shared/scene-kit`: `ec32b292` (initial lift, 11 files), `418bd23c` (the
`u_page2` channel, 2 files), `1fd64e91` (this fix, 2 files). A full
`git status --short --untracked-files=all` at the repo root, filtered only
for build-artifact `target/` noise, comes back completely empty.

## IME preedit landed (TODO.md item, closed)

`Ev::Preedit` overlay-rendered at the caret (never touches the buffer/
undo/version, matching the ghost and popup's existing pattern); IME
Commit routes through plain `Ev::Text` since committing IS a text
insertion. Read egui's own reference `TextEdit` IME handling in full
before designing this — kept its defensive "\n"/"\r" ignore rule (a
documented real platform quirk, not guessed at) and deliberately diverged
from its buffer-splicing approach (wrong fit for an editor with real undo
history and a version counter other systems watch). See `DESIGN.md`'s
matching section for the full reasoning.

## A real find bug found and fixed (minimap work, TODO.md item)

Building the scrollbar minimap surfaced a genuine pre-existing bug in
`FindState::recompute`: a manual char-offset accumulator silently dropped
each line's terminator character, so any find match past the first line
landed at a systematically-too-low global offset — worse the further into
the file. A 200-line-file end-to-end test caught a 12-line discrepancy;
short test fixtures elsewhere in the crate never accumulated enough drift
to trip an assertion, which is almost certainly why it sat undetected.
Fixed by asking the buffer for each line's true start
(`pos_to_char(Pos::new(li, 0))`) instead of accumulating manually. See
`DESIGN.md`'s matching section for the full narrative.

## Streaming ghosts landed (TODO.md item, closed)

`Provider::complete_streaming` + a real `OllamaProvider` override with
incremental chunked-HTTP reading and genuine mid-stream cancellation
(`should_cancel`, checked between every delta — a cancelled stream drops
its `TcpStream`, not just discards the eventual answer). `CompletionOutcome`
gained `is_final: bool`. See `DESIGN.md`'s "Streaming ghosts" section for
the design correction made mid-implementation (an early string-comparison
heuristic for "is this a duplicate final send" was replaced with an
explicit flag once it became clear it would make the loop pane's history
unreliable). Tested against a real local `TcpListener`, including a token
deliberately split across an HTTP chunk boundary.

## What this is

41 commits on `session/aperture-editor`, branched from `main`, building
**aperture** (a Rust/egui code editor with an AI ghost-completion loop) and
**aperture-view** (a physical/spatial 3D file viewer — "syntax tree in
space," rooms per code item, git history you can walk through). Both live
under `apps/aperture/`. A shared 3D toolkit, `shared/scene-kit`, was lifted
from `logscope-egui` early in the session and both apps' git integration
(`gitspace`) lives in `aperture-core` so both shells share it.

Started from an empty `apps/aperture/` (the repo's `manifest/projects.toml`
didn't know about it yet). By the last commit: 231 tests (176 in the
aperture workspace + 55 in `scene-kit`), zero compiler warnings anywhere in
the touched crates, `verify.sh` clean for both `aperture` and `logscope`
(the latter confirmed by an actual build, not just by argument — see the
missing-commit-audit section above).

## Merging this

```
git fetch /path/to/aperture-session.bundle session/aperture-editor
git log main..FETCH_HEAD --oneline   # the 41 commits, newest last above
git merge FETCH_HEAD                 # or rebase/cherry-pick as you prefer
```

The bundle is a **range** (`main..session/aperture-editor`), not the full
branch history — smaller, and it's what `git bundle create` needs to avoid
packing the whole repo. If `main` has moved since this bundle was made,
expect ordinary merge conflicts in `manifest/projects.toml`, `TODO.md`, and
`apps/aperture/DESIGN.md` (all edited almost every commit) — nothing
structural, just adjacent-line noise from two things growing at once.

## Orientation: what actually exists

**Two binaries**: `aperture` (flat 2D editor, crate `aperture-egui`) and
`aperture-view` (3D spatial viewer, crate `aperture-scene`). Both share
`aperture-core` (rope buffer, tree-sitter syntax, **gitspace** — the git+
gitmind integration), `aperture-complete` (the completion engine: Ollama/
subprocess/static providers, zero dependencies by design), and
`shared/scene-kit` (the 3D rendering core, shared with `logscope-egui`,
which keeps its own separate copy — not migrated onto the kit yet).

**If you only read one thing in the code**, read `aperture-core/src/
gitspace.rs`'s module doc, then `apps/aperture/crates/aperture-scene/src/
viewer.rs`'s doc comments on `jump`, `rev_step`, `enter_homecoming`, and
`current_hour`. That's where most of the session's real design decisions
live and where a bug is most likely to hide if you extend this.

**Key/command surface** (both shells mostly mirror each other):
`,`/`.` step through git history (older/newer); `Ctrl+B` branch from the
previewed revision; `Ctrl+M` materialize a worktree; `Ctrl+H` (editor) or
`H` (viewer) enter/toggle homecoming; `V` (viewer) mark "I was here";
`Ctrl+F`/`Ctrl+H` find/replace (editor only — different `Ctrl+H` meaning
in each shell, be careful); `[`/`]` (viewer) walk stations; `Ctrl+O` fuzzy
file picker (editor).

## Commits, in order, one line each

1. `17fdc47a` rope core, ghost completion, egui shell, palette
2. `ec32b292` scene-kit lifted from logscope; aperture-view's physical UI
3. `d4f73ed6` tree-sitter syntax, find, auto-pairs
4. `10383ee2` follow camera (ported from a user-supplied Atom plugin)
5. `774c500c` outline-based context filler + symbol popup
6. `418bd23c` rooms: outline → spatial corridor, EnvParams-style room mood
7. `e27134fc` gitspace: git+gitmind history the corridor can show
8. `c5b62175` git status in the flat editor; gitspace moved to `aperture-core`
9. `ebf4e689` revision preview (read-only) in the flat editor
10. `9a9dbe8d` AI margin notes on revisions, in the flat editor
11. `de33a005` branch-from-here (`Ctrl+B`), gitmind's checkpoint pattern
12. `32030d0e` branch-from-here in the viewer too
13. `afdf53ad` materialize (`Ctrl+M`) — **and a real bug**: the live-editor
    guard for `Ctrl+B` was never actually wired (silent no-op, not a
    guarded one) until this commit's test caught it
14. `312263ae` touched stations: which rooms did *this* diff change
15. `1397ce25` fuzzy file picker for `Ctrl+O`
16. `a5a89979` brace-split Enter — **and a real bug**: revision preview's
    read-only guard never covered `Enter`, so it could mutate a "read-only"
    buffer; fixed
17. `331f0458` CJK/double-width caret math (partial — named honestly)
18. `0b3a6ed2` … closed the partial scope from #17 (selection/find highlight rects)
19. `7403cca8` Find & Replace, atomic replace-all — **and a real gap**: find
    had zero visible UI before this (just an in-text highlight wash)
20. `2fbf6f80` find history (`Ctrl+↑/↓`) — **two real bugs**, one in the
    test's assumptions, one in the code's clamp-direction logic
21. `11c727f7` **Homecoming**: git-ref-based "mark where I left off," wind
    forward through the gap, rooms light up by what changed since — direct
    response to a user description of a felt need, not a TODO item
22. `c89c9f3b` sun follows the clock — logscope's day/night cycle, wired to
    real commit timestamps while previewing
23. `051a9b03` commit history as an actual graph (logscope's `graph_scene`)
    — **a real git-behavior lesson**: `git log -- path` drops merge commits
    by default; the test was rewritten around that, not worked around
24. `104292e3` rooms shaped by real signal (logscope's `region_plan`) —
    **two real bugs** in the dormancy threshold, both caught by the first
    test run before shipping
25. `8f18b184` `HANDOFF.md` (this file)
26. `d8031d00` graph-node click-to-jump — closes the gap #23/#24 left open
27. `9c37dbc1` the AI margin note's length limit now has a reason (logscope's `textsurface`)
28. `59f27b4f` the commit graph uses logscope's real `flow()` layout (time-stretch + lanes), not a placeholder single-lane one
29. `8eae21d1` `G` frames the commit graph (logscope's camera-fit vocabulary: `centroid`/`bounding_radius`/`fit_distance`), so it's reliably reachable regardless of a person's orbit habits
30. `edd17325` (no code change) verified logscope-egui itself still builds clean — real build, not assumed, after seven commits touching the shared kit it does NOT depend on
31. `4323edeb` fixed stale references inside this very file (missing
    commit hashes on items #26–30, a wrong commit number, and a deferred
    item that a later commit had actually already closed) — checked
    against `git log` directly rather than trusted from memory
32. `1fd64e91` fixed a real gap: the sun-parameterization changes from
    commit #22 never actually got committed (`cd apps/aperture && git add .`
    silently missed `shared/scene-kit`) — every bundle handed off between
    #22 and this one would have failed to compile for anyone who fetched
    it. Also added the missing `scene-kit/Cargo.lock`.
33. `b376177b` flagged the above prominently at the top of this file
34. `cff43758` audited for other instances of the same bug (found none;
    documented the audit method so it's checkable, not just asserted)
35. `af90fdfe` **streaming ghosts** (`Provider::complete_streaming`,
    Ollama `stream:true`, the cancellation path — a `TODO.md` item this
    session had explicitly named as "larger, not tackled solo") — written
    by a parallel instance of this assistant, not this thread; see the
    authorship note above
36. `81f379d7` that commit's own doc-count follow-up
37. `675941c1` closed the SAME class of gap as #32, this time in
    `TODO.md`: the parallel session's strikethrough for "streaming
    ghosts" had the identical uncommitted-file problem, caught by the
    same full-repo `git status` discipline while auditing #34–35

**Pattern worth knowing**: roughly a third of these commits found a real,
previously-existing bug or gap while building something else — not
introduced by that commit, found by writing a test that checked the actual
guarantee instead of the happy path. Each one is named explicitly in its
commit message and in `DESIGN.md`'s matching section. If you're auditing
this branch, that's where to look first.

## What's deliberately NOT done (don't rediscover these the hard way)

- **Personalized named bookmarks** (plural, distinct from Homecoming's
  single "last visited" marker) — a real, separable feature the same
  `refs/aperture/*` git-ref pattern makes straightforward. Not built.
- **`batch_by_day`** (groups revisions into day-buckets, in `gitspace`,
  tested) exists but isn't wired into any UI — no "welcome back, here's
  Tuesday's batch and Wednesday's batch" overview yet.
- ~~Graph-node click-to-jump~~ DONE (commit 26, `d8031d00`):
  `World.graph_node_ix` + a distance-based screen-space hit test (spheres
  don't have quad corners the way plaques do — a deliberately separate
  convention, not a forced reuse).
- ~~Multi-lane graph layout~~ DONE (commit 28, `59f27b4f`): the commit
  graph uses `scene_kit::spatial::flow` — logscope's own river-with-lanes
  layout, real time-stretch and real fork lanes, not pure time order.
- **Cloud provider** (`ureq`/TLS) and **infer-kit extraction** (lifting
  `aperture-complete`'s Ollama client into a crate shared with
  `logscope-fan`) were both explicitly *declined*, not forgotten — the
  former contradicts `aperture-complete`'s stated zero-dependency design,
  the latter would edit a different app's crate unprompted. Worth
  reconsidering if a real need shows up, but don't do either casually.
- **Multi-cursor, IME preedit, streaming ghosts, LSP**: named in
  `TODO.md`, untouched, larger than anything tackled solo this session.
- **`logscope-egui` itself was never modified — confirmed by actually
  building it**, not just by argument. It has NO Cargo dependency on
  `shared/scene-kit` at all (checked `Cargo.toml` directly): its own copies
  of `mesh3d.rs`/`glow_scene.rs` under `crates/logscope-egui/src/` are
  fully separate files, so nothing this session did to the shared kit could
  reach it structurally. Verified anyway with a real
  `cargo build -p logscope-egui` (clean, 2m54s, only its own 24 pre-existing
  warnings) and a full-repo `verify.sh` (both `aperture` and `logscope`
  clean). If you migrate logscope onto the shared kit later, re-check
  `TODO.md`'s scene-kit migration section — it has a running list of what
  diverged (the `u_page2` inscription channel, the parameterized sun
  functions, at minimum).

## Running things

```
cd apps/aperture
cargo build --ignore-rust-version                    # both bins
cargo test  --ignore-rust-version                     # workspace (~153)
cargo test  --ignore-rust-version -p scene-kit         # +55, tracked separately
```

`--ignore-rust-version` is required every time: egui 0.34's declared MSRV
(1.92) is ahead of the sandbox's installed rustc (1.91) — a policy floor,
not an actual API requirement, confirmed safe by every green test run this
session. No display in this sandbox: both bins boot to a clean
`WinitEventLoop` "no display" error, which is the expected, correct
outcome of a real headless smoke test — not a failure. **No visual
rendering has ever been verified.** If you have a real screen, that's the
single highest-value thing to do next: run `aperture-view` and look at it.

Disk note: this sandbox filled up more than once mid-session. The
recurring cause was stale linked test binaries (`target/debug/deps/*`,
40MB+ each) surviving across many incremental builds. If a build fails
with "No space left on device," `rm -f target/debug/deps/aperture*` before
doing anything more drastic.
