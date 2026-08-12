# Handoff — aperture session bundle

For whichever Claude picks this up next. Read this before touching code;
it's the map, not the territory — `apps/aperture/DESIGN.md` is the
territory (it's long — ~50KB, one section per commit below, each with its
own "honest ledger" of what's tested vs. compiled-only vs. deliberately not
built) and `TODO.md`'s aperture section has the standing backlog.

## What this is

24 commits on `session/aperture-editor`, branched from `main`, building
**aperture** (a Rust/egui code editor with an AI ghost-completion loop) and
**aperture-view** (a physical/spatial 3D file viewer — "syntax tree in
space," rooms per code item, git history you can walk through). Both live
under `apps/aperture/`. A shared 3D toolkit, `shared/scene-kit`, was lifted
from `logscope-egui` early in the session and both apps' git integration
(`gitspace`) lives in `aperture-core` so both shells share it.

Started from an empty `apps/aperture/` (the repo's `manifest/projects.toml`
didn't know about it yet). By the last commit: 208 tests (153 in the
aperture workspace + 55 in `scene-kit`), zero compiler warnings anywhere in
the touched crates, `verify.sh` clean.

## Merging this

```
git fetch /path/to/aperture-session.bundle session/aperture-editor
git log main..FETCH_HEAD --oneline   # the 24 commits, newest last above
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
- **Graph-node click-to-jump**: the commit graph renders but isn't
  clickable — needs a sphere hit-test convention, plaques use flat quad
  corners which doesn't transfer directly.
- **Multi-lane graph layout**: the commit graph is single-lane (pure time
  order); real DAG lane-packing (parallel branches drawn parallel) is a
  harder, separate problem.
- **Cloud provider** (`ureq`/TLS) and **infer-kit extraction** (lifting
  `aperture-complete`'s Ollama client into a crate shared with
  `logscope-fan`) were both explicitly *declined*, not forgotten — the
  former contradicts `aperture-complete`'s stated zero-dependency design,
  the latter would edit a different app's crate unprompted. Worth
  reconsidering if a real need shows up, but don't do either casually.
- **Multi-cursor, IME preedit, streaming ghosts, LSP**: named in
  `TODO.md`, untouched, larger than anything tackled solo this session.
- **`logscope-egui` itself was never modified.** Every scene-kit change
  this session was checked (by `grep`, not assumption) to confirm
  logscope's own separate, unmigrated copy of the shared files was
  unaffected. If you migrate logscope onto the shared kit later, re-check
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
