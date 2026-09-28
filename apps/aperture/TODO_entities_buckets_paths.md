# aperture — entities, buckets, paths

Reconciles three inputs into one queue: the 2026-09-11 design conversation
("Spatial Code Editor"), Kess's 2026-09-08 patch of inline TODOs
(`0001-planning-kess.patch`), and what `apps/aperture` already is at
`c17unified-090` (HEAD `5db8a71e`, 2026-09-08).

Written from reading the code at the paths named below, not from building
it. Nothing here was compiled. The one mechanical check that was run:
`git apply --check` on Kess's patch against HEAD — it applies cleanly, and
both base blobs (`23ec0062`, `994d23a6`) are present in the bundle.

Intended home: `apps/aperture/TODO_entities_buckets_paths.md`, with a
one-line pointer added under `## aperture (apps/aperture) — follow-ups` in
the root `TODO.md`. It supersedes `maintenance/C12_TODO.md` §7 (the
semantic-navigation/marker ask), which is the seed of the same idea and
should get a "see apps/aperture/TODO_entities_buckets_paths.md" line rather
than being deleted.

---

## 0. The three inputs, and where they actually meet

### The transcript

A general design conversation (game-engine examples, not this repo). Its
conclusions, stripped of the illustrations:

1. **Files are storage; the working unit is a code entity** (an AST item,
   a file, a crate, a commit, a runtime frame). Navigation moves down
   through AST containment, up through files/directories, and sideways
   through relationships (calls, callers, refs, tests, grep hits).
2. **Every query source enters one graph**: grep, tree-sitter, LSP, git,
   AI, runtime. `Query -> {nodes, edges}`. Nodes are deduplicated by a
   canonical id, and a node remembers *why* it is on screen (provenance).
3. **The navigation object is an edge, not a location**: you arrived at B
   *via callsite 17 in A*. The way back collapses into a visible path.
   Non-1:1 cases (many callers, trait dispatch) collapse on the
   relationship used, never on the destination alone.
4. **Buckets**: temporary, ordered, named collections of entities with an
   attention horizon (`max-bucket-depth`). Buckets outlive files-as-tabs;
   files reappear only as parent context when useful.
5. **TODO ropes**: a bucket with a lifecycle (question → investigating →
   structuring → resolved) and an evidence log of what the operator did.
6. **Harnesses**: an executable bucket — code + fixture + expectation —
   that can stay ephemeral or graduate into a real test. Verification
   scoped to the bucket, then reverse deps, then everything, with a
   known-good frontier over revision history.
7. **Voice produces semantic actions, not text**: one `Intent` IR that
   keys, palette, and voice all resolve into.
8. Do **not** start with 3D. Semantic gravity only in temporary regions;
   anchored things never move.

### Kess's patch (five comments, no code)

| # | site | what it says |
|---|---|---|
| K1 | `palette_ui.rs` above `command_list` | compile-time command loader, less boilerplate, zero overhead |
| K2 | `palette_ui.rs` above `apply` | replace raw timers with flags/gates; chunked reorderable ops; timers in global state inside a shared networking daemon for monorepo apps with shared workspaces; batching; pub/private packet split for compression |
| K3 | `palette_ui.rs` above `apply` | `apply` should walk an internal op list instead of repeating boilerplate |
| K4 | `state.rs` ~2277 | "use a type instead of a Key?" |
| K5 | `state.rs` ~2320 | Ctrl+D has no complement; "add previous similar" bound to B |
| K6 | `state.rs` ~3023 | root workspace concept has to learn to take file packs |

Note the repo already has a convention for these — C12_TODO §18 records
`// KESS:` review comments arriving in a patch. This patch uses plain
`// TODO`. Land it as-is with `git am` (keeps authorship — CLAUDE.md §6
prefers that over re-typing), then resolve each comment through the items
below; do not rewrite the comments to `// KESS:` in the same commit.

### What already exists (the part the transcript did not know)

| transcript concept | repo module | state |
|---|---|---|
| AST entities, containment | `aperture-core/src/syntax.rs` `outline()`, `OutlineItem{kind,name,sig,line,end_line}` | done, tree-sitter feature |
| grep as a navigation primitive | `aperture-core/src/project_search.rs` (Ctrl+Shift+F) | done |
| symbol index across the workspace | `aperture-core/src/workspace_symbols.rs` (Ctrl+T) | done, name index only |
| intent-first navigation | `aperture-core/src/route.rs` + `aperture-complete/src/route_ai.rs` (Ctrl+G) | done, two-tier |
| forward call edge | `aperture-lsp` `goto_definition` (F12) | done |
| **callers / references** | — | **absent** — no `textDocument/references`, no `callHierarchy` in `aperture-lsp/src/client.rs` |
| **jump-back / navigation path** | — | **absent** — F12 has no return; no stack anywhere in `state.rs` |
| git as a time dimension (heat, touched, history graph, revision preview) | `aperture-core/src/gitspace.rs`, `aperture-scene/src/{rooms,graph,viewer}.rs` | done |
| personal markers | `gitspace` `refs/aperture/visited/<file>` | done (one marker; named bookmarks explicitly deferred in HANDOFF.md) |
| side-by-side / portals | `aperture-core/src/panes.rs` (`DocId`, `View`, `PaneWorkspace`) | core done, **not wired**; the refactor cost is written up in `TODO_panes_and_collab.md` |
| spatial world | `aperture-scene` corridor: outline → stations, deterministic room env by kind + name hash | done, never visually verified |
| voice → command | `aperture-core/src/voice.rs` `VoiceCommand`, `AppState::run_voice_command` | text side done; every arm calls an existing method |
| modal keys as a table | `aperture-core/src/helix.rs` `resolve(mode, ch, shift) -> Act` | done, typewriter-mode only |
| multi-user | `aperture-collab` (RGA, Lamport ids, `wire.rs`) | core done, no transport, not wired |
| shared daemon | `aperture-engine::Reach::{Daemon,InProcess}` + `daemon::DaemonBackend` (model); `apps/coordd` + `shared/coord-core` (resource arbiter over a unix socket); `studio-collab/src/carrier.rs` (chunking over a lossy keyed store) | seams exist, three of them |
| buckets, ropes, harnesses, depth lens, entity registry, semantic Intent IR | — | absent |

The one-sentence reconciliation: **aperture has every edge *source* the
transcript names except callers, and no edge *layer* to put them in.**
Everything the transcript wants that is not already built — buckets,
paths, ropes, harnesses, depth lens, voice intents — is a consumer of that
missing layer. Build the layer first; it is headless, testable with the
hermetic-repo fixtures `gitspace` already uses, and it does not touch
`view.rs`.

---

## 1. Decisions

Recorded so the items in §2 are not re-derived. Each names the alternative
that was rejected.

**D1 — A headless entity/relation layer in `aperture-core`, before any UI.**
`entity.rs`: `EntityId` (stable, content-derived — see the gotcha in §4),
`Relation` (Contains, Calls, CalledBy, References, DefinedIn, TouchedBy,
IntroducedBy, TextMatch, TestedBy), `Edge{from, rel, to, at: Option<Site>}`
where `Site` is the callsite/line the edge was observed at, and a
`EdgeSource` trait implemented by the existing modules. Rejected: a new
`shared/` crate now. `shared/` has had duplication problems (C12_TODO §7's
own warning), and there is exactly one consumer. Lift it to `shared/` at
the second consumer, the way scene-kit was lifted.

**D2 — The navigation object is an edge, and there is no jump-back today.**
`nav.rs`: `NavStep{from, rel, edge_site, to}`, `NavPath` (a vec with a
cursor, so forward is possible after going back). F12, Ctrl+T, Ctrl+G,
project-search hits, and graph-node clicks all push a step. This is the
smallest user-visible win and it gates the collapse-into-path UI. Rejected:
a plain `Vec<(path, line)>` location stack — it cannot answer "how did I
get here", which is the whole point, and it cannot disambiguate two paths
that converge on the same function.

**D3 — Callers need one LSP addition; tree-sitter gets the cheap tier.**
`textDocument/references` (and `callHierarchy/incomingCalls` where the
server advertises it) in `aperture-lsp`. Same two-tier shape `route` and
the symbol popup already use: a deterministic tree-sitter tier — an
identifier occurrence inside another outline item's span is an approximate
`CalledBy` — that works with no server, and the LSP tier merged in when it
answers. Rejected: waiting for LSP only. rust-analyzer needs `rust-src` for
std and a warm-up after `didOpen` (both already found the hard way, root
`TODO.md` aperture section); the editor must not have zero caller edges
until then.

**D4 — Buckets are Kess's "file packs", and both are an ordered entity list
plus a horizon.** `bucket.rs`: `Bucket{name, entries: Vec<EntityId>, depth,
pinned: BTreeSet<EntityId>}`. `AppState::workspace_root()` (state.rs:670)
grows into a `Workspace{root, buckets, active}`; the file walker becomes one
bucket *source*, not the definition of the workspace. Persist per repo
under `refs/aperture/bucket/<slug>` — a blob of the serialized bucket
pointed at by a ref, the same "a git ref, zero new file format, nothing to
gitignore" reasoning `visited` used — with a plain-file fallback for
non-repos. Rejected: `.aperture/` directory in the tree (needs gitignore,
travels with clones, and `visited` already chose refs).

**D5 — Keep `now_ms` as a parameter. Kess's K2 is three separate asks.**
DESIGN.md: "time is always a parameter, so history coalescing, debounce and
the ghost fade are tested with fake clocks." There is one clock origin
(`view.rs:32`, `ctx.input(|i| i.time)`), 47 `now_ms: u64` parameters in
`state.rs`, and a handful of deadline fields (`status` expiry, `Debounce`,
ghost). That is not excess storage; it is the seam that makes a daemon
clock possible later. Split K2:

* *Logical ordering for chunked, reorderable ops* — already the design of
  `aperture-collab` (Lamport ids, order-independent RGA) and
  `studio-collab::carrier` (chunks, tolerates reorder/dup, no retransmit
  by design). Nothing to add to aperture; wire aperture-collab over that
  carrier when collab is wired (`TODO_panes_and_collab.md` step 4).
* *A shared daemon for monorepo apps* — `coordd` is that daemon's shape
  (unix socket, pushes directives) and `aperture-engine::Reach::Daemon` is
  the model-sharing case with a construct-and-fall-back seam. A "shared
  workspace" daemon is a `coord-core` extension or a sibling of
  `logscope-serve`'s `netserve`, not an aperture crate.
* *Batching and public/private packet split for compression* — belongs
  in `studio-collab/src/wire.rs`/`carrier.rs` if anywhere. Read `wire.rs`
  before designing; do not put a network policy in `palette_ui.rs`.

The comment sits above `apply` because `now_ms` flows through it, not
because the palette is where the work is. Say that when resolving it.

**D6 — One `Action` enum, three front doors; no proc-macro loader.**
K1, K3, K4 and the transcript's `Intent` are the same ask. Today
`palette_ui::Cmd`, the `(key, ctrl, shift, alt)` match in `state.rs`
(~40 arms at 2280–2345), `voice::VoiceCommand`, `cmdstrip`, and
`helix::Act` are five vocabularies for one set of operations. Make
`Action` the single one, with a `static` table of `(label, default_chord,
Action)` the palette, key dispatch, cmdstrip and voice all read. Zero
overhead is a `&'static [ActionSpec]`; a `macro_rules!` that emits the
enum, the labels and the `apply` arms from one list is house-compatible
(`Scope::ALL` already generates palette entries this way). Rejected: a
proc-macro crate — a build-time dependency for a table that is 40 rows.
Also fixes a real cost: `matches()` calls `command_list()` on every
keystroke and allocates every label each time.

**D7 — K5 as written collides with Ctrl+B (`branch_from_here`).** Two
different things are plausibly meant: *add previous occurrence* (needs a
backward `find_occurrence` — the current one wraps but only searches
forward) and *undo last added cursor*. Build both in `multicursor.rs`,
bind Ctrl+Shift+D and Ctrl+U (both free — see §3). Ask Kess which he
meant only if a binding preference matters; the code wants both anyway.

**D8 — Don't rebuild the corridor. Buckets and paths feed both shells.**
The transcript's "don't start with 3D" is already satisfied: `aperture`
is the flat shell and `aperture-view` is the second. Its "anchored objects
never move" rule already holds — rooms are placed by a deterministic hash
of kind + name. C12_TODO §7 asked whether `shared/nifty-core/src/
graph_layout.rs` (force-directed) fits: only for a *temporary* region
(search results, a bucket's unpinned tail), never for stations. Rejected:
a force-directed relayout of the corridor.

**D9 — Ropes and harnesses come after entities, and use what exists.**
A rope is a bucket with a lifecycle and an evidence log of `NavStep`s.
A harness is a bucket that owns a `cargo test -p <crate> <filter>`
invocation; the known-good frontier is gitmind's `refs/checkpoints/*`
timeline `gitspace` already merges, plus `maintenance/tools/break-test.sh`
for "does this harness catch anything". Rejected: a harness runner that
is not `cargo test` — Rust already has the runner and the repo already has
the checkpoint history.

**D10 — Voice resolves into `Action`.** `voice.rs`'s grammar stays (its
"a near-miss must not execute" rule is right); its output type becomes
`Action`, with a `VoiceCommand -> Action` step in the interim so nothing
about the parser changes in the same commit that changes dispatch.

**Declined or out of scope here, with the reason:**

* Runtime stack/heap tracker as a jump list — there is no run-capture
  anywhere in the tree (no probe/gdb/DAP integration). The entity layer
  should reserve `Relation::ObservedIn` and a `RunId` so it can be added,
  and that is all until a capture exists.
* AI moving a harness across the production boundary — needs harnesses.
* Semantic gravity / auto-clustering — needs buckets and a temporary
  region to cluster in; and it is a layout concern for `aperture-scene`.
* The "identity ribbon" inline UI — a rendering of `Edge`s; after D1.
* Named bookmarks (HANDOFF.md's deferred item) — becomes "a bucket with
  one pinned entry" once D4 lands; do not build it separately.

---

## 2. TODOs, in dependency order

Each: what, where it attaches, the test that fails when it is wrong, size.
"Small" is a morning; "session" is what `TODO_panes_and_collab.md` calls a
session. Commit one concern each (CLAUDE.md §8); an item that says "two
commits" means it.

### A. Land Kess's patch — small, one commit

`git am 0001-planning-kess.patch`. Comments only; nothing to build. Then
each `// TODO` below gets a one-line trailer naming the item here that
resolves it, in a second commit, so `grep -n TODO palette_ui.rs` points
somewhere. Do not delete the comments until the item is done.

### B. `aperture-core::entity` — DONE

`0004-aperture-entity-graph-and-nav.patch`. `EntityId` (File / Item / Crate
/ Commit / Extern / Frame, with a total `key()`/`parse_key()` round trip),
`Relation` with `inverse()`, `Edge` carrying `Site` + sources + `approx`,
the `EdgeSource` trait, and `Graph` with merge-not-replace, `neighbors`
(inverting incoming edges), `why()` provenance, `expand()`, and
`neighborhood`/`neighborhood_via` — which is item J, so J is done too.
`cargo test -p aperture-core` 202/202, break-tested.

Reserved and unused: `EntityId::Frame` and `Relation::ObservedIn`, so a run
capture is a new source later rather than an enum change every persisted id
must survive.

### C. Edge sources — three DONE, two left

`0005-aperture-edge-sources.patch`. `OutlineEdges` (C1, containment),
`SearchEdges` (C2, text matches attributed to the enclosing item), and
`GitEdges` (C3, blame folded to items by max line, plus per-file
`IntroducedBy`). 213/213, six of six breaks caught.

They take plain `OutlineRow`s rather than calling `Syntax::outline`, so the
nesting and attribution logic tests in the featureless build. The single
conversion point is `OutlineRow::from_items`, which is the only place
`OutlineKind` becomes a string — and note it compiles ONLY under
`tree-sitter`, which could not be built here (`tree-sitter 0.25` →
`indexmap 2.14` → edition2024, refused by cargo 1.75). Someone with the
real toolchain should build `--features tree-sitter` once before trusting
it.

Still to do:

4. `goto_definition` → `Calls` with the caret's `Site`. Needs the LSP
   worker; belongs with D.
5. Approximate tree-sitter `CalledBy`: identifier occurrences of item X's
   name inside item Y's span, `.approx()`. The graph's merge rule is
   already built for this — an exact LSP answer at the same site clears the
   flag, and there is a test pinning that a later guess cannot un-clear it.

### D. `aperture-lsp`: references and call hierarchy — DONE except the real-server run

Landed 2026-09-27: `LspClient::{references, incoming_calls, callers}`,
capability-gated (refused unsent when not advertised), a fifth worker
channel with its own generation, and Shift+F12 in the shell (one answer
jumps, several open a `Callers` palette list, one row per call SITE; the
jump is a `CalledBy` / `ReferencedBy` nav step). Proven over real stdio
against `aperture-lsp/examples/lsp_stub.rs` (`tests/stub_server.rs`),
including a server->client request reusing an in-flight id, which the
reader used to take as our reply. NOT yet run against rust-analyzer: the
test is written (`callers_finds_a_cross_file_caller_through_a_real_rust_
analyzer`) and skips without one. Nothing feeds the answers into
`entity::Graph` yet — that waits for a consumer (item N).

`textDocument/references` (with `includeDeclaration: false`) and
`callHierarchy/prepareCallHierarchy` + `incomingCalls`, gated on the
server's advertised capabilities the way `supports_incremental_sync()`
already gates range sync — refuse rather than send to a server that did
not advertise it. Real proof, same as every other LSP feature here: a
cross-file caller in the fixture package through a real `rust-analyzer`,
with the retry-on-empty the F12 test already needed. Then a fifth
`lsp_worker.rs` outcome channel (own generation counter — the reason the
fourth one has its own is written in root `TODO.md`).

### E. `NavPath` + jump-back — DONE (core, editor, viewer)

Shell half landed (2026-09-27): steps are pushed by F12
(`jump_to_lsp_definition`, relation References) and every palette accept
(`AppState::palette_jump`: picker = `Opened`, project search = `TextMatch`,
symbols/route = `References`). Alt+Left / Alt+Right, placed above every
other Left/Right arm with a test that fails if reordered, plus ‹ › chips
and a breadcrumb in the status bar (no Alt key on a phone). Core grew
`NavStep::landing` (forward needs a position, `to` is only an entity) and
a narrower self-loop rule (same entity AND same line), because file-level
entities made every same-file F12 a dropped self-loop. Sites carry the
absolute path: `workspace_root()` is per-file outside a repo. Found on the
way: `open_file` silently discarded unsaved edits on every file switch
(F12, picker, search hits); it now refuses once and discards on the same
request repeated. Viewer half also landed: plaque clicks and graph-node
jumps push steps (`[`/`]` do not — walking in order is its own way back),
←/→ walk them, and a station is re-found by kind+name in whichever
revision is showing.

Core is in the same patch as B. `NavStep{from, rel, to, site}`, `NavPath`
with a cursor (back returns the STEP, so the callsite comes with it),
browser-style forward truncation, bounded at 256 dropping the old end,
`breadcrumb(max)` and `visited()`.

Shell half still to do, unchanged from the original plan: push a step in
`jump_to_lsp_definition`, workspace-symbol accept, route accept,
project-search accept, and `open_file` from the picker. Bind Alt+Left /
Alt+Right — and note the dispatch match is on `(key, ctrl)` with `if alt`
guards, so `(Left, false) if alt` must sit ABOVE the existing
`(Left, false)` arm or it is unreachable with no compiler warning. That
hazard is now demonstrated rather than asserted: patch 3 has a shell test
that fails when the Ctrl+Shift+D arm is ordered wrongly, and the Alt/Left
pair should get the same treatment. `aperture-view`'s `jump_to_graph_node`
and station clicks push too; `Viewer` takes the same `NavPath`.

### F. `Action` unification — session, at least three commits

1. `action.rs` in `aperture-egui`: the enum and `static SPECS:
   &[ActionSpec{label, chord: Option<Chord>, action}]`. `Cmd::Theme`/
   `SetScope` become `Action::Theme(PaletteId)` etc. generated from
   `::all()` into a `LazyLock<Vec<_>>` beside the static slice.
2. Palette reads `SPECS` (no per-keystroke allocation); `apply(Action)`
   is the one dispatcher; `palette_ui::apply` becomes a shim.
3. Key dispatch: the `(key, ctrl)` match becomes `chord_to_action(chord)
   .map(apply)` for the arms that are plain commands. Motion/edit arms
   (`moved`, backspace, enter, the ghost/popup pre-checks at 2190–2275)
   stay hand-written — they take shift/selection state the table does
   not, and forcing them into it is where a table stops being clearer.
   Say which arms moved in the commit.
4. `VoiceCommand -> Action`; `run_voice_command` shrinks to a parse +
   `apply`. Its 10 tests keep passing unchanged — that is the check.
5. `cmdstrip::Cmd::new(&label, &label)` (view.rs:1616) takes the spec's
   label, so recents and the palette cannot drift.

Test that fails when wrong: every `ActionSpec` with a chord is reachable
from the key path and the palette path and produces the same state
(iterate `SPECS`, assert both routes, assert the count ≥ 30 so an empty
table cannot pass). Break-test it: remove one arm, watch it fail.

### G. Multicursor complement — DONE (core), shell wiring unverified

`0003-aperture-multicursor-complement.patch`. `find_occurrence_back`,
`add_prev_occurrence`, `pop_cursor`; Ctrl+Shift+D and Ctrl+U in the shell.
`cargo test -p aperture-core` 177/177, break-tested. Resolves K5 —
answering it as BOTH readings ("add the previous" and "undo the last
cursor"), since the code wants both and only the binding was in doubt;
Kess's suggested `B` is `branch_from_here`.

Two notes for whoever reviews it. `find_occurrence_back` is a forward scan,
not `doc[..cut].rfind` — rfind misses a match that starts before the cut
and ends after it, and widening the slice to compensate can land
mid-codepoint. And `K` had no `U` variant and `view.rs` no `EK::U`
mapping; without both the binding compiles and is simply dead, which is
worth checking for any future letter binding.

### H. Buckets in core, and the workspace takes packs

**The core half is DONE** — `0001-aperture-core-buckets.patch`, applies
clean to `5db8a71e`, `cargo test -p aperture-core` 169/169 (was 154),
break-tested. `aperture_core::bucket` has `Entry`/`Bucket`/`Workspace`/
`locate`/`slug`, a total `key<TAB>value` format, a `Store` seam, and
`gitspace::GitStore` over `refs/aperture/bucket/<slug>` (blob refs;
gc-survival, `rev-list --all`, and bundle behaviour all measured against
a real repo — see the commit message). `Entry` is path + optional symbol
name, resolved by name so it survives the item moving; it grows an `id`
field when B lands, without a format break.

Correcting what §4 says below: a blob ref **is** carried by
`git bundle create --all` (it shows in `git bundle list-heads`) but is
**not** created by `git clone` of that bundle — clone takes heads and
tags only. Fetch explicitly, exactly like `refs/notes/toolcalls`
(CLAUDE.md §1). Measured, not assumed.

**Shell half landed (2026-09-27)**, except the `workspace_root()` change:
Ctrl+K opens a `Bucket` palette (list / filter / type a new name to make
one; inside: "+ Add <item under caret>", "+ Keep the jump trail" from
`NavPath::visited`, entries resolved by name with `locate` — unresolved
ones listed with "renamed?", never dropped — a ✕ per entry, and a delete
that asks once). Storage is `GitStore` in a repo, else a `FileStore`
under `$XDG_DATA_HOME/aperture/buckets/<slug of root>` (never a file in
the project). Saved on every bucket change. Jumps from a bucket are nav
steps. Needed `K::LetterK` (a variant named `K` is ambiguous under the
`use K::*` in `key()`) plus the `EK::K` mapping.

Found on the way and fixed: `open_file` never re-discovered the repo or
the file's history — `git`, `revs` and the diff gutter stayed the FIRST
file's, so Ctrl+, after Ctrl+O loaded the old file's revisions into the
new buffer, and a session started on a scratch buffer never knew it was in
a repo (so `workspace_root()` was the file's directory).

Still to do, shell side:

* `AppState::workspace_root()` (state.rs:670) returns a `PathBuf`;
  make it a `bucket::Workspace`. `file_finder::walk_files` becomes one
  entry source, `panes::open_docs()` another, a saved `NavPath` a third.
  Resolves K6 properly — the core patch only makes it possible.
* Palette mode `Mode::Bucket` (Ctrl+K — free): list buckets, enter one
  to list entries, `+` adds the item under the caret. `locate` wants
  `(kind, name, line)` triples, which `Syntax::outline()` already
  produces; the mapping is three lines and belongs in the shell so
  `bucket.rs` stays feature-free.
* Load on open, save on change. Do NOT save per keystroke — a bucket
  write is a `hash-object` plus an `update-ref`, two subprocesses; save
  on bucket mutation and on quit, the cadence the diff gutter uses.
* `depth` is stored and ignored until J.

### I. Panes step 1 → bucket pane — session, already scoped

`TODO_panes_and_collab.md` step 1 ("`AppState` gains `panes` and a
`BTreeMap<DocId, Editor>`; no UI change") is a prerequisite for showing a
bucket as a pane, and its cost note (230 `.editor` sites, borrow splits,
not renames) is still the right estimate — `state.rs` has grown to 4,899
lines since it was written, so re-count before starting. Do that step
unchanged. Then a bucket pane is `View` showing a bucket instead of a
doc: entries in order, each an inline portal into its span (the
transcript's mobile answer, and the transcript is right that the editing
focus stays singular — the bucket pane is read/jump, edits happen in the
doc pane it jumps to).

### J. Depth lens — core DONE, no UI

`Graph::neighborhood_via(root, max_depth, allow)` landed with B, with the
monotone-in-depth and shortest-hop properties tested. What is left is
purely presentational: bind a bucket's `depth` to what the bucket pane and
the corridor show, let a `NavPath` step temporarily raise the bound along
its own edge, and put `depth 2 · 24` in the status bar.

### K. Boundaries as entities — small, after D

`EntityId::Crate{name, version}` and `EntityId::Std{path}` from LSP
`goto_definition` targets outside the workspace root; `Relation::
DocumentedBy` pointing at `~/.cargo/registry/src/.../` and `rustc --print
sysroot`'s `rust-src` when present (it was absent in the sandbox — say
"no docs available" rather than fail). The hover overlay already flattens
`hover` text; a boundary entity's card is that text plus the in-workspace
`CalledBy` list ("where else do we use this"), which is the transcript's
"link back to other uses".

### L. Ropes — session, after H and E

`Rope{bucket, stage: Question|Investigating|Structuring|Resolved,
evidence: Vec<NavStep>, notes: {known, likely, unknown, todo}}`. A rope is
created from a `// TODO` under the caret (`Site` anchored to the enclosing
`EntityId`, not the line). Evidence appends automatically from `NavPath`
while the rope is active. Persist with the bucket. The four-bucket note
shape is the transcript's own and it matches the house rule about not
letting guesses become facts; keep it. Test: a rope's evidence after a
scripted jump sequence is exactly that sequence.

### M. Harnesses and scoped verification — session, after L

`Harness{rope, package, filter, last: Vec<(oid, Pass|Fail)>}` running
`cargo test -p <pkg> <filter>` through the same subprocess shape
`CommandProvider` uses. Scope ladder in the status bar: this harness →
bucket's crates → reverse deps (`cargo metadata` gives them) → workspace.
Known-good frontier: the newest `refs/checkpoints/*` oid whose harness
passed, from the merged timeline `gitspace::history` already builds.
"Graduate to test" = write the harness body into `tests/` with the rope's
notes as the doc comment. Do not run a harness on every edit; debounce it
on save, the way the diff gutter refreshes.

### N1. Caret policy — DONE (core), shell wiring unverified

`0002-aperture-caret-policy.patch`, applies clean to `5db8a71e` and
independent of patch 1. `camera::CaretPolicy` returns alpha plus the next
wake; `Solid`/`Blink`/`Breathe` in the palette. Kills the only continuous
timer in the app. `camera.rs` verified standalone 14/14 and break-tested;
`state.rs`/`view.rs`/`palette_ui.rs` are compile-unchecked because
aperture-egui does not build on the toolchain available here. First thing
to check: `ctx.input(|i| i.focused)` is the egui 0.34 spelling from
memory. First thing to tune, on a screen: `breath_ms` 1600, `dim` 0.2,
`settle_ms` 8000 are guesses.

### N2. Lifting from logscope-egui — investigated, three separate answers

**First, a stale claim to fix.** `apps/aperture/HANDOFF.md` says
logscope-egui "has NO Cargo dependency on `shared/scene-kit` at all
(checked `Cargo.toml` directly)". It does now —
`logscope-egui/Cargo.toml:24`, `default-features = false`, with a comment
saying which modules migrated. The migration TODO.md §scene-kit describes
is partly done. Correct HANDOFF.md before anyone plans around it; this is
CLAUDE.md §5 happening on schedule.

**a. Physical UI — mostly already shared, one clean lift left.**
`shared/scene-kit` (spatial, mesh3d, gpuscene, glow_scene, texgen,
inscribe, textsurface) is the physical UI core and aperture-scene already
uses all of it. What did NOT come across is
`logscope-egui/src/physui.rs` — 672 lines, an in-scene control FSM
(Momentary / Toggle / Selector / Dial), and its own doc says "pure +
testable: pointer events and the picked id go in, actuation events and a
visual state come out — no egui, no GPU". aperture-scene has two ad-hoc
hit-test conventions already (plaque quad corners; graph-node screen
distance) and no press/release discipline at all: a click actuates on
down, so dragging off a plaque still jumps. `physui` is the fix and it is
a straight lift — pure, no egui, testable here. Do it as its own commit
into `shared/scene-kit` with re-export shims, the route the kit was
lifted by. Companions worth reading at the same time:
`nodelabel.rs` (550) and `plaque_scroll.rs` (163).

**b. Theming — do NOT lift. Aperture is already on the shared path.**
`logscope-egui/src/theme.rs` is 2,451 lines with a THREAD-LOCAL active
palette and `BG()`/`INK()` free-function accessors. Aperture uses
`uikit_egui::theme_bridge::apply_palette` + `snapclone_theme`'s 14
palettes, passed as a `PaletteColors` value. That is the better shape and
the fleet-wide one; adopting a thread-local global would be a regression
and would fight the headless tests. Two things in that file are worth
taking as IDEAS rather than code, and both are mobile-relevant:
`Material` (Paper/Wood/Stone/Marble as a palette property, which
aperture-scene's plinths already have as geometry but the flat shell does
not), and `power_mode()`/`effective_variant()` — the weak-GPU
auto-detect. Note the trap already recorded in root TODO.md's 2026-09-05
section: that auto-detect silently overrules the quality dial and
presented to a user as "the textures disappeared". If aperture ever grows
a quality dial, make the override VISIBLE, which is the fix that section
asks for and nobody has done.

**c. The character morph — take the discipline, not the pearl.**
`logscope-egui/src/pearl_input.rs` (257 lines) is the composer as a field
of glowing 5×7 cells with real temporal stability: a persistent pool
keyed by `(char slot, glyph x, glyph y)`, diffed on every edit, so shared
cells hold their position and twinkle phase while removed ones disperse
and new ones coalesce in. Its only non-`std`/`egui` dependency is
`glyph5x7`, which is already in `scene_kit::mesh3d` — so the lift is
mechanical.

Three things to decide before lifting it, in order of how much they cost:

1. **The pearl chrome is the exact thing patch 2 just removed.** Every
   frame it stacks 8 bloom rects, a 16×5 facet web at 5 blur passes each,
   and 150 film-grain circles reseeded at 12 Hz, and it animates
   unconditionally. Take the pool; leave the chrome, or give it the same
   settle rule the caret now has.
2. **The key is a SLOT INDEX, so a mid-line insert re-morphs everything
   after it.** Fine for a chat composer you mostly append to; wrong for a
   code editor, where typing at the start of a line would dissolve and
   re-coalesce the rest of it. Aperture has the primitive logscope
   lacks: `aperture-collab`'s RGA gives every character a Lamport id that
   is stable under insertion. Keying the pool on that instead of on the
   index is what makes this idea work in an editor at all, and it is a
   real argument for wiring collab's `Doc` under the buffer sooner than
   `TODO_panes_and_collab.md` step 4 assumes.
3. **5×7 cells are not readable code.** What transfers to aperture is the
   pool-diff behaviour applied to real font glyphs at their galley
   positions, not the dot-matrix look. The best target is not the
   composer — aperture has no composer — it is the **streaming ghost**,
   which today replaces its whole text on every delta. A stable pool
   would make a growing suggestion settle instead of re-flowing, and it
   is adjacent to the interaction bug root TODO.md documents but did not
   fix (accepting part of a streaming suggestion gets reintroduced by the
   next delta). Same underlying gap: nothing tracks glyph identity across
   two versions of the ghost. Do that after E and F; it wants a real
   screen, not a test.

### N. aperture-view consumes buckets and paths — trail DONE, bucket wing not

The lit trail landed (2026-09-27): `Viewer::trail_stations` resolves the
path behind the cursor to this corridor's stations by kind+name, and
`scene::build_rooms_with_trail` lays glowing floor tiles between them and
brightens visited plaques. Tested on the mesh list; NOT seen on screen
(aperture-view needs GL, which the sandbox this was written in lacks).
The bucket wing is not started: it needs stations from OTHER files in a
viewer that holds one buffer, which is a real change to `Viewer`.

A bucket is a wing (its entries as stations in bucket order); the active
`NavPath` is a lit trail between stations. Uses `frame_points` for
"frame this bucket". No layout change to the main corridor (D8).

---

## 3. Keybinding ledger

From `state.rs` 2280–2345 at HEAD. Ctrl unless stated.

| taken | by |
|---|---|
| Z / Shift+Z / Y / A / S | undo / redo / redo / select all / save |
| O / P / T / G / F / Shift+F / H | open / palette / symbols / route / find / project search / replace |
| E | follow camera |
| , / . | older / newer revision |
| B / M | branch from here / materialize |
| I | hover |
| Space / Shift+Space | popup / complete now |
| D / Shift+L / Alt+Up / Alt+Down | next cursor / all occurrences / column cursors |
| Left/Right/Home/End | word / doc motion |
| F3, F12 (no Ctrl) | find next / go to definition |

| proposed | for | item |
|---|---|---|
| Alt+Left / Alt+Right | nav back / forward | E |
| Shift+D | previous occurrence | G |
| U | undo last cursor | G |
| K | bucket palette | H (taken, 2026-09-27) |

Free after that: J, N, R, W, Q, Shift+K, Shift+U. Ctrl+Shift+V-family is
not usable — egui delivers Ctrl+C/V/X as clipboard events before they are
key events (found and recorded under the voice item in root `TODO.md`).
aperture-view's bindings are plain letters (`[ ] H V B G`), so mirror
with unmodified keys there.

---

## 4. Gotchas the next session will hit

* **`EntityId` and renames.** A content-derived id changes when the
  function is renamed, and then every bucket entry and rope anchor
  pointing at it is orphaned. Do not "fix" this with a UUID map in v1 —
  it just moves the problem to "who updates the map". v1: on load,
  re-resolve each entry; an unresolved entry is shown as such, not
  dropped (the "unknown is not dormant" rule from `rooms`). v2 can use
  git's rename detection through `gitspace` to rewrite ids on commit.
* **Borrow splits in `state.rs`.** `TODO_panes_and_collab.md` is exact
  about this and it applies to every item that adds a field the key
  dispatcher and `editor` both touch. Add fields on a sub-struct (`nav`,
  `buckets`) and take `&mut self.nav` early, the way `extra_cursors`
  avoids it.
* **LSP capabilities.** `references` is nearly universal; `callHierarchy`
  is not (rust-analyzer yes, many others no). Gate on
  `capabilities.callHierarchyProvider`, and remember the warm-up: every
  e2e test here retries once after `didOpen`. Adding request kinds to
  `client.rs` means adding them to the `initialize` capabilities you
  advertise, or some servers will not answer.
* **`didChange` and the approximate caller tier.** The tree-sitter tier
  recomputes on the open buffer; the LSP tier answers for the *synced*
  buffer. After the typing-sync bug (root `TODO.md`: ordinary keystrokes
  did not send `didChange` for a long time), do not assume they agree —
  show `approx` edges distinctly until the LSP answer lands.
* **Refs for persistence — measured, and the answer is two-sided.**
  `export-bundle.sh` uses `--all`, and a blob ref does ride along in the
  bundle. It does not survive a `git clone` of that bundle, because
  clone takes heads and tags. So a bucket is present in the pack and
  invisible in the working clone until someone runs
  `git fetch <src> 'refs/aperture/*:refs/aperture/*'` — the identical
  trap CLAUDE.md §1 documents for notes. `git rev-list --all` skips
  non-commit refs rather than erroring, and `git gc --prune=now` keeps
  the blob; both checked in a scratch repo.
* **`--ignore-rust-version`** on every cargo invocation if the toolchain
  is < 1.92 (egui 0.34 MSRV). `cargo ailite check`, one crate at a time;
  the aperture target tree is ~3 GB and does not coexist with a second
  egui app in the sandbox.
* **`cargo test --lib` is not the suite.** Every LSP e2e and the
  snapshot tests live in `tests/`. Run plain `cargo test` before quoting
  a count.
* **Nothing in aperture has ever been looked at on a real screen** per
  HANDOFF.md; Xvfb + `HEADLESS_GUI_TESTING.md` is how the multicursor
  bugs were found. Items E, F and H change what is on screen; screenshot
  them.

---

## 5. What was and was not verified writing this

Read: `CLAUDE.md`, `apps/aperture/{DESIGN,HANDOFF,TODO_panes_and_collab}.md`,
root `TODO.md` aperture and 2026-09-05 sections, `maintenance/C12_TODO.md`
§7/§18/§22, `palette_ui.rs` in full, `state.rs` 175–230 / 2255–2345 /
3020–3060, `multicursor.rs` 198–216, `panes.rs` public surface,
`aperture-engine/src/lib.rs` 1–60 / 295–345 / 512–530, `command_bar.rs`
head, `studio-collab/src/carrier.rs` head, `coordd/README.md`, and the
transcript's user turns plus its architecture, phase, edge, registry,
lens, rope, frontier, harness and voice sections.

Run: `git apply --check` (clean), `git bundle list-heads`, `grep` for the
keybinding ledger and the `now_ms` count (47), `find` for
`graph_layout.rs`.

Not run: any cargo command. Line numbers are HEAD's and will drift the
moment item A lands (the patch adds 15 lines above some of them).
