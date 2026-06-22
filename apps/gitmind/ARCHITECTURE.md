# gitmind — architecture & roadmap

Touch-first, advanced repo manager. Built in the style of the `web2qml`
reference: `qmetaobject` 0.2 + QML for the UI, Vulkan as the universal
GPU target, a pure testable core with thin frontends, heavy rationale
comments, hand-rolled where a dependency isn't worth it.

`gitmind` is a placeholder name.

## Workspace

```
gitmind/
├── crates/
│   ├── core/   pure Rust, no Qt — backend trait, chunk store,
│   │           checkpoint engine, ingest, merge. The heart.
│   ├── cli/    scriptable `gitmind` binary (buildable today)
│   ├── gui/    qmetaobject + QML touch app  (next)
│   └── tui/    ratatui terminal frontend     (next)
```

The core is Qt-free and unit-tested, so `cargo test -p gitmind-core`
runs anywhere. Each frontend is a different *binary* over the same core
— that is what makes the tool "separable as a tui and cli" rather than
one binary with feature flags.

## The three load-bearing designs

### 1. Chunk store (`core/src/store.rs`) — solves size + compression + the media DB

Files are streamed through content-defined chunking (FastCDC), each
chunk BLAKE3-addressed, new chunks zstd-compressed into an embedded redb
database, duplicates stored once. A file becomes a small manifest: an
ordered list of chunk addresses.

- **No 1 GB ceiling** — ingest is *streaming*, bounded memory regardless
  of file size. The limit is disk, not RAM or a hardcoded cap.
- **Better compression than git** — git zlib-deflates each binary
  version whole; we dedup at the chunk layer *before* zstd, so a large
  asset re-exported with a small change re-stores only changed chunks.
- **"A database for storing those files efficiently"** — that database
  is redb: transactional, mmap'd, pure-Rust (no libsqlite/C).

### 2. Media policy — AV1/AVIF as proxies, never silent lossy replacement

Originals of ingested photos/video are **always** stored losslessly via
the same chunk path. With the `media-proxy` feature on, we *additionally*
generate an AV1 (video) / AVIF (stills) proxy for fast preview and
scrubbing, referencing the original. Lossy replacement of versioned
source-of-truth bytes is never the default — it would be unrecoverable
data loss. If you ever want it, it's an explicit, per-ingest opt-in.
Encoder path: libaom / SVT-AV1, GPU-assisted via Vulkan/VAAPI where the
hardware supports it.

### 3. Checkpoint engine (`core/src/checkpoint.rs`) — checkpoints, autocommits, backups, timeline

Snapshots the whole worktree into a **shadow ref** namespace
`refs/checkpoints/<session>` as a normal commit chain, **without
touching the user's index or HEAD** (private `GIT_INDEX_FILE` →
`write-tree` → `commit-tree` → `update-ref`). From this one mechanism:

- **better autocommits** — a `notify` file watcher calls `snapshot()` on
  a debounce; identical trees are skipped so the timeline stays clean.
- **aggressive branching with checkpoints** — promoting any checkpoint
  to a real branch is one `branch <name> <ckpt-oid>`.
- **backups** — push `refs/checkpoints/*` to a remote/USB mirror.
- **code timeline overviews** — `list()` over the chain feeds a
  scrubbable timeline UI.
- **worktree side-by-side** — `materialize()` checks any checkpoint or
  branch out into a fresh worktree next to the others (this is also
  where "file prefixing in directories" lives: prefix each worktree's
  dir with its branch/checkpoint label).

## git backend (`core/src/backend.rs`)

All git ops go through the `RepoBackend` trait. Default `GitCli` shells
out to `git` — the reference's deliberate choice: full ecosystem compat
(hooks, config, credentials), ~5–20 ms/query, no libgit2 build burden.
A future `Hybrid` backend implements the same trait and intercepts the
large-file paths into the chunk store, with zero frontend changes.

## On "GPU-native git" — where the speed actually is

Git is pointer-chasing and I/O-bound; it is not a data-parallel workload,
so a GPU rarely helps the graph/index/delta paths (PCIe transfer alone
eats the win). The real, already-in-place speedups are CPU-side:

- BLAKE3 (SIMD, tree-parallel hashing) instead of SHA-1.
- zstd instead of zlib.
- rayon across files, mmap, streaming.

Vulkan compute is reserved for where it genuinely pays: **media
transcode** (AV1/AVIF proxies) and **batch hashing** of many chunks at
once. This matches the reference's "Vulkan everywhere" stance without
pretending the graph walk belongs on a GPU.

## Smarter merging (later phase)

Tree-sitter / structural 3-way merge (mergiraf-style): resolve
reorder/format-only conflicts automatically by diffing syntax trees, not
lines. Falls back to git's merge driver when no grammar is available.

## Touch UI mapping (gui crate, next)

Built on the reference's `GestureHost` contract — one `MouseArea` owns
press→drag→release→commit/cancel, the popup is a passive visualizer.

| Gesture                 | Operation                                  |
|-------------------------|--------------------------------------------|
| Swipe file row right    | stage (`backend.stage`)                    |
| Swipe file row left     | unstage (`restore --staged`, reversible)   |
| Long-press file row     | radial menu: discard / ignore / blame / history |
| Pinch on graph          | zoom commit/timeline graph                 |
| Pull down               | fuzzy file finder (nucleo)                 |
| Two-finger swipe        | switch panel (Status · Timeline · Branches)|

### Zip autoingest wizard (AI coding sessions)

A watched drop-folder or picker feeds a wizard whose state machine lives
in core: **pick zip → preview extracted tree → choose target (new branch
/ subdir / fresh worktree) → detect session metadata → extract + auto-
commit to a checkpoint → done**. Media inside the zip flows through the
chunk store; code lands on a checkpoint branch ready for an AI session.

## Phased roadmap

1. **Core foundation** *(this commit)* — chunk store + checkpoint engine
   + backend trait + CLI. `cargo test -p gitmind-core`.
2. **GUI skeleton** — `Backend` QObject, QML shell, live status panel.
3. **Touch staging** — swipe/long-press wired to stage/unstage/discard.
4. **Timeline + graph** — checkpoint timeline, commit DAG, pinch-zoom.
5. **Ingest wizard** — zip → AI-session flow over the chunk store.
6. **Media proxies** — AV1/AVIF generation behind the feature.
7. **Hybrid backend + smarter merge** — chunk-store large files,
   tree-sitter merge, worktree side-by-side, backups.
8. **TUI** — ratatui frontend over the same core.

## Open decisions (need your call)

- **Lossy media**: confirm proxies-keep-originals is the default
  (recommended). Opt-in lossy replacement is easy to add but dangerous.
- **Store location**: `.git/gitmind/store.redb` (travels with repo) vs a
  global content store shared across repos (better dedup, more coupling).
  Current code uses the per-repo path.
- **Checkpoint cadence**: debounce interval + retention/GC policy for the
  shadow refs (e.g. keep all for 24 h, then thin to hourly).
- **Name**: `gitmind` is a placeholder.
```

## Subrepo composition (compose.rs)

Cross-repo code movement for vendoring libraries and field-testing
experiments, built on content addressing.

- **Bubble-up hashing** (`tree_oid`): a directory's id is the BLAKE3 of
  its sorted children's ids, folded recursively. A deep change bubbles up
  to a new root; identical subtrees in *different repos* get the *same*
  id, so a vendored library dedups in the global store and an
  auto-created subrepo has a stable cross-repo identity. (Verified: deep
  edits change the root; identical subtrees under different parents match.)
- **Graft** (`graft`): insert another repo's tree under a `prefix/` on a
  fresh branch as one commit — drop an experiment into a mature crate or a
  library into a new project, then test it in that branch. Core git
  plumbing only (`read-tree --prefix` in a throwaway worktree); the user's
  checkout, index, and HEAD are never touched. (Verified on real repos.)
- **Subrepo autocreation** (`snapshot_subrepo`): extract a subdir into a
  standalone repo (content-only in v0; history-preserving split later).

Roadmap: this is phase 7.5 — it rides on the chunk store (global dedup)
and the checkpoint engine (graft onto a checkpoint branch to field-test
without ceremony).

## Workflow & query layer (built this iteration)

- **History search** (`query.rs`): by author (`--author`) and/or language
  (extension pathspec). "What did Alice change in Rust" is one query.
- **Review sessions** (`review.rs`): the answer to "no staging area" — work
  freely, then *mark a review*. Marking summarizes work since the last
  review (files, authors, languages, line counts), tags it
  (`refs/reviews/<name>`), and fires hook scripts (env-passed summary) for
  CI-like tasks. Index/HEAD untouched.
- **File-write time machine** (`watch.rs`): per-write snapshots for a
  curated set of code/text types only, debounced, into shadow refs — so
  the visible log never churns and media is excluded (it's ingested).
- **Similarity / near-dup** (`similarity.rs`): chunk-overlap similarity
  powering 99%-match media tracking (see MEDIA.md).
- **Distribution** (`remote.rs`): rclone (cloud incl. Proton Drive) and
  rsync/ssh command builders, plus `SshDistributor` to auto-push selected
  branches to LAN hosts from the commit backend. Command construction is
  unit-tested; exec is a thin layer.

Test count: 18 unit + 9 real-git-repo integration = 27 green.

## Performance: borrowed from git (maintenance.rs)

git already solved the hard performance problems for the parts we keep as
git; we just turn them on, and they happen to accelerate features we built:

- **commit-graph + generation numbers** → fast timeline/ancestry walks.
- **changed-path Bloom filters** → fast path-limited log, which is exactly
  `query.rs` filtering history by language. Free speedup for search.
- **packed-refs** → essential for us: every checkpoint and review is a
  ref, so we'd otherwise drown in one-file-per-ref. `pack-refs --all`.
- **repack** → delta-compress loose objects.

Deliberately NOT borrowed: git's pack delta-format (we use CDC + zstd in
the chunk store for big/binary files, which git packs poorly), and
sparse-index / partial-clone (frontend checkout-scaling, a later phase).
Verified on real repos: commit-graph file written, checkpoint refs packed.

## Predictive media encoding (encode.rs)

ffmpeg-driven, segment-based, with a queue and pass/fail hooks:
- **Segments** (keyframe/GOP-aligned for video, whole/tiled for images) so
  only changed segments re-encode, then outputs stitch.
- **Predictive fingerprint** ("fake hashing"): a cheap sampled u64 per
  segment predicts change without a full read; an "unchanged" verdict is
  confirmed by a real BLAKE3 before being trusted, so speed never costs
  correctness. `segments_to_reencode()` drives incremental re-encodes as
  edits roll in.
- **Encode queue** with Queued→Encoding→Pass/Fail and bounded retries; a
  validation hook (ffmpeg decode-to-null) marks pass/fail and re-queues
  bad encodes. ffmpeg invoked via tested command builders.

## Email / patch-series interop (patch.rs) — b4 overlap

b4 (0.15.x) wraps git format-patch/send-email and applies series with
`git am` (its `shazam -H` does a 3-way merge), and its new `review` TUI
tracks per-series branches by lifecycle. Our overlap, added now:
- `format_series` (produce a series), `apply_series` (apply, 3-way capable
  like shazam -H), `parse_mbox` (preview a series before applying).
- An applied series becomes a **review session** (review.rs) — the same
  per-series tracking b4 review does, reusing machinery we already have.
- Deferred: fetching threads from lore by message-id (a public-inbox/lei
  HTTP call), trailer collection, PGP attestation.

## Project broadcast (announce.rs) — toward the DHT

`ProjectCard` is a content-addressed project descriptor (name,
description, root oid, advertised refs, provider endpoints). Two keys:
`name_key` (discover by name) and `id` (fetch an exact, tamper-evident
card). `providers` reuse the RemoteTarget seam to say where the data
lives. The DHT (Kademlia) and hosting-provider HTTP impls are the deferred
transport; ed25519 signing lands with the encryption phase. Verified:
stable id, edits change id, name key stays constant.

## CLI, frontend dispatch, and git interop (this iteration)

- **Frontend dispatch** (`launch.rs`): the one `gitmind` binary detects its
  context — terminal vs piped, $DISPLAY/$WAYLAND, SSH, touchscreen
  (/proc/bus/input), on-screen keyboard (squeekboard/maliit), and mobile OS
  (postmarketOS/Mobian/Droidian via /etc/os-release) — and picks CLI vs GUI.
  `--gui/--cli/--tui` and `GITMIND_FRONTEND` always win. SSH is always CLI.
  Decision logic is pure + unit-tested; GUI launch is stubbed until the gui
  crate exists (it falls back to CLI status with a note).
- **CLI ergonomics**: forgiving help (`help`, `-h`, `-help`, `--help`),
  `-v/--version`, short aliases (`st`, `snap`, `tl`, `gc`), and a clean
  single-column status (the old double `??` is gone). All built core
  features are now wired as subcommands: search, review, maintain, graft,
  ingest, restore, import, bundle, where.
- **`-f` fuzzy selection** (`select.rs`): list commands take `-f` to emit a
  coded list (a0, a1, … b0 …); interactively it hands off to `fzf` if
  present, otherwise reads a selection string. Grammar: `c3` (single),
  `a3..b1` / `a3-b1` (inclusive range, order-independent), `a0,c3` (multi),
  `*` (all). Selected oids print one per line so `gitmind tl -f | xargs …`
  composes. Parser is unit-tested; the arrow-key TUI picker is the tui crate.
- **git interop** (`portage.rs`): the native store now lives in `.gitmind/`
  (separate from `.git`). `import` streams every reachable `.git` blob into
  the `.gitmind` chunk store (deduped, bounded memory) — the "ingest a .git
  and build an equivalent" path. `bundle` exports a `git bundle --all` that
  stock git can clone, for full ecosystem compatibility. Both verified on
  real repos (re-import dedups to zero; git verifies the bundle).

Scope note: `import` currently brings the **blob data** into `.gitmind`;
importing the full commit/tree graph and operating entirely without `.git`
(native refs in `.gitmind`) is the next portage phase. Checkpoints and
reviews are still git refs today.

Test count: 35 unit + 12 real-git-repo integration = 47 green.

## GUI crate + FFI bridge + peek (this iteration)

- **gitmind-gui** (qmetaobject + QML, like web2qml): one `Backend` QObject
  registered as the `backend` context property, QML embedded via `qrc!`,
  loaded with `engine.load_file("qrc:/qml/main.qml")`. Touch UI: swipe-to-
  stage rows (`FileRow` SwipeDelegate), checkpoint `Timeline`, and a
  `PeekViewer` file pane. The reference's `GestureHost` pattern is reused
  for custom pan/scrub.
- **FFI bridge**: every `Backend` method calls gitmind-core (the same code
  the CLI runs) and returns JSON the QML parses — a tiny bridge surface and
  zero CLI/GUI drift. The viewer calls the **peek** library directly.
- **peek** is now a workspace crate: a `view` library (mmap + newline index,
  bucket/`@symbol`/context selection) used by the GUI, plus the `peek` and
  `peek2` bins in the toolbox.
- **Toolbox + cross builds** (TOOLBOX.md): `cargo build --release` produces
  every binary except the Qt GUI (excluded from default-members). Pure-Rust
  bins cross-compile via `cross`; the GUI builds natively on a Qt6 host
  (its only blocker — confirmed the Rust compiles up to the Qt link step).

Verified: peek refactor compiles + tests; gui is static-clean and its build
reaches the Qt system-dependency step (qttypes/qmake), i.e. Rust is sound.
Toolbox test count: 50 green (core 35+12, peek 3).

## Zip ingest summary, provenance, and the netcat bridge (this iteration)

- **Zip ingest summary** (`ingest_zip.rs`): `plan_unzip` is a dry run of an
  overwriting extract — it recognizes, per entry, ADDED / MODIFIED /
  UNCHANGED by **content hash** (not size/mtime), so an `unzip -o` that would
  rewrite identical bytes is correctly a no-op. The plan IS the wizard's
  summary; `apply_unzip` performs it (zip-slip-safe). CLI: `gitmind zip
  <archive> [--apply]`. Tested: a re-extract of identical content reports
  zero changes.
- **Process provenance** (`provenance.rs`): attribute a change to the actor
  behind it — AiAgent / BuildTool / Editor / VCS / Shell. Two sources: a
  self-reported `GITMIND_AGENT` env (unprivileged, e.g. an AI tool sets
  `GITMIND_AGENT=claude`), or a PID resolved via `/proc` (the privileged
  fanotify path — plain inotify can't see the writer). Snapshots get tagged,
  e.g. `[ai-agent:claude] …`, and `conflict_needs_human` flags AI-vs-human
  collisions on shared files while letting build-tool churn auto-resolve.
- **Netcat partner bridge** (`bridge.rs`): `tar | nc` pipelines to stream a
  whole folder to a peer for ad-hoc pair coding — `gitmind bridge
  send|recv`. Explicitly the low-tech, plaintext, trusted-LAN-only path; the
  durable encrypted sync stays in remote.rs.
- **SIGPIPE fix**: the CLI resets SIGPIPE to default so `gitmind tl -f | fzf`
  / `| head` terminate quietly instead of panicking on a broken pipe.

Test count: 60 green (core 45 unit + 12 integration, peek 3).

## Realtime watcher (watcher.rs) — the time machine, built out

The `notify`-backed event loop that turns the watch policy into actual
autosnapshots:

- Watches the worktree recursively; filters events through `WatchPolicy`
  (code/text types only, never media or `.git`/`.gitmind`).
- Debounces bursts (editor save-storms) into one snapshot via a settle
  window (`--debounce`, default 1500 ms).
- Snapshots into `refs/checkpoints/<session>` — shadow refs, so the visible
  log never churns ("no realtime log updates").
- Provenance-tags only when a process self-reports (`GITMIND_AGENT`);
  otherwise a clean `autosave` label. (inotify can't see the writing PID;
  that needs the privileged fanotify path, designed in provenance.rs.)
- Clean shutdown via a stop channel; `run_forever` for the CLI.

CLI: `gitmind watch [--debounce MS] [--types rs,py]`. Verified live — edits
produce autosnapshots, and `GITMIND_AGENT=claude gitmind watch` tags them
`[ai-agent:claude] autosave`. The watcher loop is exercised by a real
integration test (spawn → write file → assert snapshot), confirming inotify
delivery end to end.

Test count: 62 green (core 46 unit + 13 integration, peek 3).

## fanotify watcher (fanotify.rs) — real PID attribution

The privileged realtime watcher. Where inotify (watcher.rs) sees a change
but not the writer, fanotify reports the **PID that wrote the file**, so each
autosnapshot is attributed to the actual process — resolved via /proc to a
command name and classified by provenance.rs (AI agent / build tool / editor
/ shell). Different writers in one session get different attributions.

- Marks the repo's **mount** for `FAN_CLOSE_WRITE | FAN_MODIFY`, resolves
  each event fd via `/proc/self/fd`, attributes by PID, filters to the repo +
  watch policy, debounces, and snapshots with the real writer's origin.
- Behind the **`fanotify` feature** (opt-in): it needs `CAP_SYS_ADMIN` and is
  Linux-only, so default and cross builds are unaffected. The stub returns a
  clear error and the CLI **falls back to inotify** automatically.
- `gitmind watch --fanotify` (build with `--features fanotify`).

Verified LIVE in this environment (root + CAP_SYS_ADMIN, kernel 6.18): a
write by a process named `claude` produced `[ai-agent:claude] autosave`, a
shell write produced `[shell:sh] autosave`, in the same session — and the
unit test confirms fanotify reports the writer's exact PID. Without the
feature, `--fanotify` degrades to inotify with a notice.

This is the conflict-resolution leverage from earlier: shared-file changes
are now attributable to AI-vs-human-vs-tooling at the moment they happen.

Test count: 62 default (core 46+13, peek 3) + 1 fanotify (feature-gated).

## Watcher: GUI integration, poll fallback, and smart fanning (this iteration)

- **PollWatcher fallback** (watcher.rs `run_with_mode`): both notify backends
  are boxed behind one loop. `--poll` (CLI) or `poll=true` forces periodic
  scanning for sandboxes/network FS where inotify is missing or silently
  broken. Verified live: poll-mode autosnapshots fire on a real edit.
- **.gmignore smart fanning** (gmignore.rs): a gitignore-flavored matcher
  (comments, globs, anchored `/`, dir `name/`, `!` negation, last-match-wins)
  restricts the watched filespace. Loaded once at watch start and applied in
  BOTH the inotify and fanotify relevance filters. Verified live: writes to
  `target/` and `*.tmp` were ignored while a watched `a.rs` edit snapshotted.
- **GUI integration** (gitmind-gui): the Backend runs the watcher on a
  background thread, pushing checkpoints into a shared queue; QML drains it on
  an 800 ms timer, updates the timeline, and flashes a banner showing the
  provenance-tagged label. Header switches: **watch** (start/stop) and **attr**
  (fanotify PID attribution). fanotify failure surfaces as a UI message and
  the user can retry without it. `cargo build -p gitmind-gui --features
  fanotify` for the privileged path.

Test count: 66 default (core 50 unit + 13 integration, peek 3) + fanotify.

## WatchSession: the GUI watch logic, made testable (this iteration)

The start → drain → stop → running state machine the QML drives used to live
inside the Qt `Backend` (uncompilable without Qt, therefore untested).
Extracted to `watch_session.rs` in core: `WatchSession` owns the background
watcher thread and the checkpoint queue, exposing `start(root, policy,
session, fanotify, poll)`, `stop()`, `drain() -> DrainResult`, and
`running()`. The Qt Backend is now a thin JSON wrapper over it.

Payoff: the exact control flow the touchscreen relies on is unit-tested
headlessly — a real session starts, an edit is drained as a checkpoint, and
stop winds the thread down (`start_drain_stop_lifecycle`), plus a
double-start no-op guard. fanotify init failures surface through `drain` as
an ("error", msg) item so the UI can show them and retry without fanotify.

Test count: 68 default (core 52 unit + 13 integration, peek 3) + fanotify.

## Time-machine recovery: diff + restore (this iteration)

The watcher could record and list snapshots, but there was no way to get data
back out — the loop was open. Closed it:

- `CheckpointEngine::diff(from, to)` — per-file line deltas of a checkpoint
  against another revision, or (to=None) the current working tree. The
  "what changed in this snapshot" view.
- `CheckpointEngine::extract(oid, dir)` — recover a snapshot's full tree into
  a plain directory via `git archive | tar` (no linked-worktree bookkeeping,
  unlike `materialize`). Returns the file count.

CLI: `gitmind diff [<a>] [<b>]` (default: latest checkpoint vs worktree) and
`gitmind restore-checkpoint <oid> <dir>`. Verified live end to end: snapshot →
timeline → diff (showed app.rs +2 −1) → restore recovered the exact earlier
content. The GUI Backend exposes `checkpoint_diff_json` / `checkpoint_extract`,
and the QML timeline now shows a snapshot's diff when a row is tapped.

So the full loop is closed: watch → autosnapshot → browse timeline → diff →
restore. Test count: 69 default (core 52 unit + 14 integration, peek 3) + fanotify.

## Native graph import: .gitmind stands alone (this iteration)

`import` previously brought only blob *data* across; now it imports the FULL
git object graph so `.gitmind` is a complete, walkable, reconstructable
mirror with no `.git` dependency.

- Store gains an OBJECTS table (git oid → kind + BLAKE3 blob oid, content in
  the chunk store so trees/blobs dedup across commits) and a REFS table
  (branches/tags + symbolic HEAD).
- `portage::import_graph` enumerates every object (`cat-file
  --batch-all-objects`), streams each into the store, and records the graph +
  refs. `native_commit`/`parse_commit` walk the DAG straight from the store
  with no git process.
- `portage::export_to_git` reconstructs a git repo from the native store
  alone, writing each object back with `git hash-object -w` and asserting the
  recomputed oid equals the stored one — so reconstruction is **bit-exact**.

CLI: `gitmind import` (full graph; `--blobs-only` for the old behavior) and
`gitmind export-native <dir>`. Verified live: a 2-commit repo imported (9
objects), rebuilt from `.gitmind` alone, identical `git log`, and the HEAD oid
matched exactly. The round-trip integration test asserts native walk + fsck +
matching HEAD/commit-count.

Test count: 70 default (core 52 unit + 15 integration, peek 3) + fanotify.

## `gin` alias + streaming import (this iteration)

- **`gin` alias**: the CLI is now a thin lib (`gitmind_cli::run`) behind two
  binaries — `gitmind` and `gin` — so `gin` is a first-class short alias with
  zero code duplication. Help reflects the invoked name.
- **Streaming graph import**: `import_graph` now drives a single
  `git cat-file --batch-all-objects --batch` process and parses the framed
  `<oid> <type> <size>\n<content>\n` stream, instead of spawning git per
  object. Same bit-exact result (round-trip test unchanged), far fewer
  processes on large histories.

## Reachable-only import (this iteration)

`import_graph` gained a `reachable` mode. Default `gitmind import` now walks
only objects reachable from refs (`rev-list --objects --all` piped into
`cat-file --batch` via a writer thread so large lists don't deadlock the
pipe) — a clean mirror of live history. `import --all` keeps every object
including dangling/unreachable ones; `--blobs-only` is the old blob-data path.
Verified live: a repo with a dangling blob imports 3 objects reachable vs 4
with `--all`, and a test asserts the dangling object is excluded by default
and present under `--all`.

Test count: 71 default (core 52 unit + 16 integration, peek 3) + fanotify.

## Native operations: .gitmind as a source of truth (this iteration)

gitmind can now CREATE history directly in `.gitmind`, with no `git` process:

- `gitobj.rs` builds git objects and computes their oids (SHA-1 over
  "<type> <size>\0<content>"), with git's directory-aware tree ordering.
  Tests pin the well-known empty-blob/empty-tree oids AND assert a
  built blob/tree/commit is **byte-identical to git's** for the same content
  (compared against `git hash-object`/`mktree`/`commit-tree`).
- `native.rs` snapshots a directory into native blobs/trees and writes a
  commit: `commit_worktree(store, dir, branch, name, email, msg)` advances a
  native `refs/heads/<branch>` and `HEAD`. `resolve_head` walks the symbolic
  ref. Empty dirs are omitted (git's rule); .git/.gitmind are skipped.
- CLI: `gin native-commit -m <msg>` and `gin native-log`.

Proven live: two commits made into `.gitmind` with git never invoked (git
reports "no commits yet"), then `export-native` reconstructs a real git repo
whose `git log` shows both — with the same oids. The round-trip test asserts
HEAD == native oid, clean fsck, and matching file content.

Honest scope: v1 commits the whole worktree (skipping only .git/.gitmind);
.gitignore honoring, symlinks, and merges are future work. But the core
claim now holds — `.gitmind` is a writable, git-compatible object store, not
just a mirror.

Test count: 77 default (core 56 unit + 18 integration, peek 3) + fanotify.

## Ignore-aware native commits (this iteration)

Native commits now honor ignore rules so their file set matches git's.
`GmIgnore::load_all(root)` merges the repo-root `.gitignore` and `.gmignore`
(gmignore wins on conflict), and `native::build_tree_from_dir` skips any
entry the matcher ignores (files and whole directories), still omitting empty
trees. The definitive test asserts a native commit's tree oid EQUALS git's
own `HEAD^{tree}` for a worktree containing ignored files (`target/`, `*.log`)
— identical oid ⇒ identical file set and content. Verified live: a native
commit exported to git contained only `.gitignore` and the tracked source,
with `target/` and `*.log` excluded just as git does.

Scope note: v1 reads only the repo-root ignore files; nested `.gitignore`
files and git's global excludes aren't yet honored.

Test count: 78 default (core 56 unit + 19 integration, peek 3) + fanotify.

## Native status/diff: working tree vs .gitmind, no git (this iteration)

`native::status(store, root)` compares the working tree to the native HEAD
commit with zero git calls: `flatten_tree` walks the native HEAD tree into
path → blob oid, `hash_worktree` hashes the live files to git blob oids
(honoring .gitignore/.gmignore, storing nothing), and the two maps are
diffed into sorted Added/Modified/Deleted changes. CLI: `gin native-status`
(aliases `nstatus`, `nst`). Verified live: clean right after a native
commit, then `D a.rs / M app.rs / A c.rs` after an edit, delete, and add —
all computed from `.gitmind` plus the worktree, never touching `.git`.

With this, the native loop is self-contained: `native-commit` → edit →
`native-status` → `native-commit`, fully independent of git, with
`export-native` as the bridge back whenever you want a real `.git`.

Test count: 79 default (core 56 unit + 20 integration, peek 3) + fanotify.

## Native checkout: navigable history without git (this iteration)

`native::checkout(store, target, dir)` materializes a native commit into the
working tree. `target` is a branch name or a commit oid (full, or a unique
short prefix as printed by `native-log`, via `resolve_commit_oid`).
`materialize_tree` restores each blob from `.gitmind` and writes it with the
right exec bit; tracked files present in the old HEAD but absent from the
target are removed (and empty dirs pruned), while untracked files are left
alone. HEAD moves — symbolic for a branch, detached for an oid. It's a force
checkout (local edits to tracked files are overwritten).

CLI: `gin native-checkout <branch|oid>` (aliases `ncheckout`, `nco`).
Verified live: from c2 (`a.txt=v2`, `b.txt`), `nco <c1-short-oid>` restored
`a.txt=v1` and `keep.txt` and removed `b.txt`; `nco main` returned to c2 —
full bidirectional time travel, entirely from `.gitmind`.

The native verb set is now commit / log / status / checkout — a usable VCS
loop with export-native as the bridge to real git.

Test count: 80 default (core 56 unit + 21 integration, peek 3) + fanotify.

## Native branching + cherry-pick, and a touch Branches panel (this iteration)

gitmind is a standalone alternative to git, so the native (git-free) layer is
where the real VCS lives. Added:

- **Branching** (`native.rs`): `branch_list` / `branch_create` /
  `branch_delete` over `refs/heads/*`; `advance_head` moves the current
  branch (symbolic) or detached HEAD; `resolve_ref_or_commit` accepts branch
  names and short/full oids. `native-commit` now targets the *current* branch
  (follows HEAD) instead of always "main" — a real bug caught in live testing.
- **Cherry-pick** (`native.rs`): file-level 3-way. It applies the change a
  commit made vs its first parent onto HEAD: clean when HEAD still matches the
  pick's base for each touched file, `Empty` when already present, and an
  honest `Conflicts(paths)` (committing nothing) when both sides changed the
  same file differently. Helpers `flatten_tree_full` (mode-aware) and
  `build_tree_from_map` synthesize the new tree; the new commit is git-identical.
- **CLI**: `gin native-branch [name|-d name]`, `gin cherry-pick <oid>`
  (auto-syncs the worktree to the resulting HEAD).
- **GUI**: Backend gained `branches_json`, `branch_create/delete/switch`,
  `cherry_pick`, plus `native_commit`/`native_log_json`/`native_status_json`.
  New `Branches.qml` touch panel: tap a branch to switch, swipe to delete,
  "+ new branch" to create, and **long-press a branch to cherry-pick its tip**
  onto the current branch. Wired as a tab in the main SwipeView.

Tests prove: branch create/list/delete (incl. refusing to delete the current
branch), a clean cross-branch cherry-pick that preserves the target's own
change, the already-applied (Empty) case, and conflict detection. Verified
live via `gin`: full branch + cherry-pick flow with zero git.

Honest scope: cherry-pick is file-level, not line-level — overlapping edits to
one file are reported as conflicts rather than auto-merged. No rebase/merge
yet (next). GUI is static-clean only (needs Qt6 on the target device).

Test count: 82 default (core 56 unit + 23 integration, peek 3) + fanotify.

## Native rebase + drag-to-rebase, and QML tooling (this iteration)

- **Rebase** (`native.rs`): refactored cherry-pick onto a shared
  `apply_commit_delta` (file-level 3-way against any base map). `rebase(branch,
  onto)` finds the commits unique to `branch` (after the common ancestor via
  `first_parent_chain`), replays them onto `onto`'s tip building a fresh commit
  chain, drops commits that become empty, and aborts on the first conflict
  (moving nothing). CLI: `gin rebase <onto>` or `gin rebase <branch> --onto
  <onto>`. Verified live: feature (f1,f2) rebased onto main (m1) → linear
  history f2→f1→m1→base, worktree synced.
- **GUI**: Backend `rebase(branch, onto)`. `Branches.qml` is now fully
  gestural — tap=switch, long-press=cherry-pick tip, **drag one branch onto
  another = rebase** (DragHandler payload + DropArea), × to delete, + to create.
- **QML tooling** (`tools/qml_check.py`): a qmllint-lite static checker since
  Qt can't run in CI here. Validates bracket/string balance, duplicate ids,
  element/type resolution against a builtin allowlist + local components, an
  import heuristic, and a qrc/dependency graph that flags a referenced
  component missing from the `qrc!` embed list. Run:
  `python3 tools/qml_check.py crates/gui/qml --qrc crates/gui/src/main.rs`.

The native VCS verb set is now: commit, log, status, checkout, branch,
cherry-pick, rebase — a genuinely usable git-free workflow, with export-native
as the bridge to real git.

Test count: 83 default (core 56 unit + 24 integration, peek 3) + fanotify.

## Patch export / apply: unified diffs without git (this iteration)

New `diff.rs`: a dependency-free LCS line differ + unified-diff engine.
`unified_hunks`/`file_patch` generate git-apply-compatible patches (proper
`diff --git`, `--- /+++`, `@@` headers, new/deleted-file modes); `apply_hunks`
applies a unified diff with strict context matching and a small offset search.
Trailing-newline handling is explicit (the phantom-empty-line bug that first
made `git apply` reject our output is fixed).

`native.rs` adds `format_commit_patch(commit)` (what a commit changed vs its
first parent; binary changes noted, not diffed) and `apply_patch_to_dir(patch,
dir)` (create/modify/delete). CLI: `gin patch <commit> [-o file]` and
`gin apply <file>`. GUI Backend: `commit_patch`, `apply_patch`.

Validated both directions with git as an oracle (never in the code path):
our exported patch passes `git apply --check` and our applier reproduces the
commit's content; and our applier correctly consumes a `git diff` patch.
Verified live: `gin patch` → `gin apply` and `git apply` both reproduce the
change.

Native verb set is now: commit, log, status, checkout, branch, cherry-pick,
rebase, patch, apply — a self-contained git-free VCS, with export-native as
the bridge to real git.

Test count: 87 default (core 58 unit + 26 integration, peek 3) + fanotify.

## Merge: completing the native verb set (this iteration)

`native::merge(theirs)` does a file-level 3-way merge into the current branch:
finds the merge-base along first-parent chains, then per file takes the side
that changed (or both when they agree), flags a conflict when both changed
differently, fast-forwards when HEAD is an ancestor, and otherwise writes a
two-parent merge commit. CLI: `gin merge <branch>`. GUI: Backend `merge`, and
a "merge" button on each branch row in `Branches.qml` (drag still = rebase,
hold = cherry-pick).

Tests cover a non-conflicting 3-way (two-parent commit, both changes present),
a real conflict, and a fast-forward. Verified live: merging `feature` into
`main` produced a merge commit carrying both sides' edits.

The native VCS verb set is now complete: commit, log, status, checkout,
branch, cherry-pick, rebase, merge, patch, apply — all git-free, with
export-native bridging to a real `.git`.

Test count: 88 default (core 58 unit + 27 integration, peek 3) + fanotify.
