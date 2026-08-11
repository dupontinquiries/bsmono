# aperture — design

The prompt was: *"a atom/vscode/zed/pulsar/kate successor in rust and egui
with AI autocomplete and smooth camera and animations that is lightning fast,
using the monorepo for code reuse and suite integration."* This document is
that prompt expanded into the decisions actually taken, so the next session
(or the next person) argues with choices, not with a vibe.

## Vision, sharpened

An editor whose three signature behaviors are each **one legible mechanism**:

1. **Fast** = O(viewport) frames. A rope for edits, a per-line lexer cache for
   color, and a renderer that lays out only visible rows + 2 overscan. File
   size never appears in the frame cost.
2. **Smooth** = one motion primitive. Scroll, caret, and font-zoom all move
   through the same critically-damped `Smooth` (exponential decay,
   framerate-independent, overshoot-free — unit-tested for both properties).
   "Camera feel" is two rate constants, not a zoo of tweens.
3. **AI you can see** = the loop pane (F2). Every completion request, its
   latency, and its outcome (or error) is a row; the provider, host, model,
   and token budget are editable fields applied live. The ghost is a
   three-state machine (`Idle → Pending{generation} → Shown`) whose every
   transition is a tested method. No black box: the pane *is* the loop.

Non-goals for v0, chosen consciously: LSP, tree-sitter, multi-cursor, tabs /
splits, plugins, IME, CJK-correct caret x, cloud TLS providers. Each has a
seam waiting for it (see TODO.md §aperture) — the point of v0 is that adding
them later touches one module, not the design.

## Architecture: three crates, dependency-poor on purpose

```
aperture-core      rope Buffer, invertible Op, grouped History, Editor,
  (dep: ropey)     LineLexer + SyntaxCache            — no I/O, no UI, no time
aperture-complete  CompletionRequest/Outcome, Debounce, Engine (worker
  (deps: none)     thread, newest-request-wins), providers: ollama FIM /
                   subprocess / static demo           — no UI, time as a param
aperture-egui      state.rs (the brain: bindings, ghost interplay — egui-free,
  (egui, eframe,   fully headless-tested), view.rs (the only module that
   uikit-egui, …)  draws), camera, ghost, palette, loop pane, eframe boot
```

**Syntax is a façade over two backends.** `Syntax::new(lang)` picks
tree-sitter when the feature is compiled and the language has a grammar
(Rust, v0), else the hand lexer — same `spans_for(line, &buffer)` either way,
so the renderer cannot tell. The tree-sitter arm (`ts.rs`) replays the
buffer's `TextEdit`s into `Tree::edit` and reparses with the old tree
(O(damage), not O(file)); spans come from a direct `QueryCursor` over the
grammar's own highlights query, restricted to the wanted line's byte range,
resolved per byte (later captures win — the query's most-specific-last
convention), memoized per line until the next edit. Deliberately not taken:
the `tree-sitter-highlight` crate — injections and locals are its whole
value, a single-language editor needs neither, and a direct cursor is a
tenth the surface. `TokKind` gained `Fn` and `Type`: the two kinds a line
lexer *cannot* produce, which makes the grammar's win visible in one glance.

Rules that keep the seams honest: **time is always a parameter** (`now_ms`),
so history coalescing, debounce, and the ghost fade are tested with fake
clocks; **the view may not contain editing logic** — if a behavior can't be
reached from `state.rs` tests, it doesn't belong in `view.rs`. That inversion
is why this editor has a real headless test story without `egui_kittest`.

**The buffer is also the change recorder.** Every `Buffer::insert/remove`
logs a `TextEdit` — byte offsets + (row, byte-col) points, recorded *before*
the rope mutates. That is tree-sitter's `InputEdit` shape and LSP's
incremental-sync shape, captured once at the only place all mutations pass
(ops, undo, redo — everything bottoms out there). `Syntax::absorb` drains the
log after each gesture; a consumer that never drains is bounded by a
truncating tail. Lapce derives the same records from rope deltas with a
factor/transform dance; recording at the mutation site is what having our own
buffer buys.

**Edit as a value** (pattern from `apps/studio`): every mutation is an
invertible `Op`; undo/redo is folding inverses; typing coalesces into
gestures by a 900 ms window + char-class boundary. Accepting a ghost is just
`insert_text`, so *undo of an AI acceptance is ordinary undo* — one Ctrl+Z
removes the suggestion, the next removes your word. Collab/macros later reuse
the same value.

**Completion correctness is one integer.** `Editor.version` bumps on
mutation only (not caret moves); a request carries it as `generation`; an
outcome is shown only if its generation equals the *current* version. Stale
answers can't ghost a buffer that moved on — they still land in the loop
pane, labeled, because seeing the misses is the point. The engine's worker
drains its inbox and serves only the newest request (`engine.rs` test pins
this with a gated provider), so a slow model never queues up a backlog.

**Ghost feel**: typing the suggestion's own next characters *consumes* them
(no flicker, no re-request); divergence dismisses and re-arms the debounce;
Tab accepts all, Ctrl+→ accepts a word (newline+indent counts as one word so
repeated taps walk multi-line suggestions), Esc dismisses. Multi-line ghosts
render first line + a `⏎ +N lines` chip — honest about size without
reflowing text below.

## Reuse map (what the monorepo bought)

- `shared/uikit/uikit-egui`: `theme_bridge::apply_palette` (all 14 snapclone
  palettes; syntax colors derive from `PaletteColors`, so theming the editor
  = theming the suite), `fuzzy::score` for the Ctrl+P palette (same ranking
  feel as gitmind's pickers), `motion::ease` for the ghost fade,
  `scroll::area` in the loop pane, `desktop::viewport` for boot.
- `apps/logscope/logscope-fan/ollama.rs`: the dependency-free raw-TcpStream
  ollama client — adapted, credited, extended with FIM `suffix`, generation
  options, and chunked-transfer de-framing (an editor can't rely on the
  lucky Content-Length case). Second call site → future `shared/infer-kit`.
- `apps/studio`: the invertible-Op pattern (pattern reuse, not a dep).
- App conventions from `bo-suite-egui`: `[lib]` + `[[bin]]` with
  `app` feature default-on, eframe `glow`+`wayland`+`x11`, nested workspace,
  manifest entry (`runtime = "none"`).

## Providers

`Provider` is a blocking trait on a worker thread (simplest thing that can't
jank the UI). Three impls: **ollama** (default; env `APERTURE_OLLAMA`,
`APERTURE_MODEL`, default model `qwen2.5-coder:1.5b-base` — a base FIM model,
*instruct models produce chat, not code continuations*), **command**
(`APERTURE_CMD`; stdin gets `prefix NUL suffix`, stdout is the completion —
any script is a provider, which is the cloud escape hatch without TLS in the
binary), and **demo** (offline static table, so first run shows the whole
loop working with no server). Swapping is live via `Engine::swap_provider`.

## Verification status (honest ledger)

- **Compiled + tested, 47 green**: aperture-core (22), aperture-complete
  (10), aperture-egui state/camera/ghost/palette headless (14), plus one
  integration test driving the *real* Engine worker thread + demo provider
  end to end (type → debounce → request → outcome → ghost → Tab → undo).
  The tests caught three real defects during the session: an EPIPE masking a
  provider subprocess's stderr, a race in the coalescing test's own
  synchronization, and an engine-presence check sitting at the debounce
  instead of the send site.
- **Compiled only**: view.rs painting, eframe boot — no display in the build
  sandbox, so "the window opens and looks right" is unverified here.
- **Unverified end-to-end**: a live ollama roundtrip (no server in sandbox);
  the wire format is pinned by tests against the documented API instead.

## Gotchas hit (so the next session doesn't)

- **egui 0.34.x declares MSRV rustc 1.92** (whole family: ecolor, emath,
  epaint, epaint_default_fonts, egui-winit, egui_glow, eframe — and pinning
  0.34.2 doesn't help, the floor is per-minor). Ubuntu apt tops out at
  rustc 1.91. It compiles fine on 1.91 with `--ignore-rust-version` — the
  floor is policy, not syntax — but the flag must ride every cargo
  invocation (build *and* test). On a ≥1.92 toolchain, drop it.
- epaint 0.34 pulls `vello_cpu` — a heavy compile; first builds are slow,
  `profile.dev opt-level=1` is set in the workspace.
- eframe with `default-features = false` still *locks* `egui-wgpu`/wgpu in
  Cargo.lock (optional deps lock even when off) — ignore the lockfile bulk,
  it doesn't build.
- glow + `wayland`,`x11` features (not wgpu) is the deliberate mobile
  choice: PinePhone/A64-class GPUs are far happier on GLES than on Vulkan
  via wgpu.
- `caret x = col × char_w` assumes true monospace: correct for ASCII,
  wrong for CJK double-width (egui does no full shaping). Galley-measured
  x is the fix when it matters (TODO).
- tree-sitter deferred specifically because grammar crates version-pair
  with the runtime and churn their `Language`/`LANGUAGE` symbol — pin both
  sides together when adding.
- **tree-sitter 0.25 API drift**: `QueryCursor::captures` returns a
  `streaming_iterator::StreamingIterator` (add the crate; `while let
  Some(..) = it.next()`, not a for loop); `parse_with` became
  `parse_with_options(cb, old_tree, None)`. Grammar crates bridge through
  `tree-sitter-language`, so runtime/grammar skew *compiles* — pin and bump
  the pair together anyway (0.25 + tree-sitter-rust 0.24 here). Grammars are
  C: `cc` at build time. `Point.column` is a BYTE column; lapce ships a
  wrong one (distance to next line start) without visible damage — evidence
  tree-sitter leans on byte offsets — but ours is computed correctly and a
  multibyte test pins it.
- **eframe 0.34 App trait**: `ui(&mut self, ui: &mut egui::Ui, frame)` is
  now the *required* driver; `update(ctx)` is deprecated but still called
  (runner calls update then ui each frame). bo-suite-egui's "real update +
  empty ui" pattern therefore works but rides the deprecation — migrate it
  when touching that app next. New `logic()` hook runs even when the window
  is not visible.
- **egui 0.34 panel API**: `SidePanel`/`TopBottomPanel` are deprecated
  aliases of a unified `Panel::left/right/top/bottom`; `default_width` →
  `default_size`; ctx-attached `.show(ctx, …)` is deprecated for
  `.show_inside(root_ui, …)` — panels now nest in the root `Ui` that
  `App::ui` hands you. `CentralPanel` survives, also via `show_inside`.
- **egui 0.34 fonts access**: `ctx.fonts(|f| …)` yields an immutable
  `FontsView`; `row_height`, `glyph_width`, and `layout_job` all need
  `ctx.fonts_mut(|f| …)` now (glyph caching mutates).
- `snapclone-theme::PaletteId::all()` has 14 entries; anything that hardcodes
  9 (older docs) is stale.

## Rooms (aperture-view): the file as a place

The ask: logscope-style scene views for code — open a file, walk its syntax
tree in space, never get lost. The mechanism is logscope's `EnvParams`
pattern (parameters in → deterministic procedural room character out, see
scene3d.rs ~6090) fed by CODE shape instead of conversation shape:

- **Stations.** Every outline item (the same `outline()` the context filler
  uses) becomes a station down a corridor along −z: a plinth in its kind's
  material, a name plaque, `mod` items as gates spanning the corridor —
  thresholds, not rooms. Alternating sides; 15 units apart.
- **RoomEnv.** Kind sets the wing's character (fn = warm workshop,
  struct/enum = cool stone, trait = violet library, impl = amber forge,
  mod = fog-thick gate, const = desaturated), an FNV-1a hash of the NAME
  tilts hue and light so sibling fns still differ. Deterministic always,
  never random — same file, same rooms, every time, because that's what
  makes spatial memory work. Jumps crossfade the env through one `mix`
  Smooth; `parse::header` is always the cool blue room past the stone arch.
- **One lectern that MOVES.** Jumping rebuilds the world (station counts are
  small; per-frame only the page's UV window slides) with the reading stand
  at the focused station. Rejected: a lectern per station — the renderer has
  one dynamic page texture, and the reader's habits shouldn't relocate.
- **Two inscription channels.** Plaques sample `u_page2`
  (`MAT_INSCRIPTION2 = 6.0`), a STATIC atlas rasterized once per file,
  beside the dynamic `u_page` the lectern re-uploads on every scroll step —
  split because the upload cadences differ by orders of magnitude. Atlas
  rows are 14 columns × 2 glyph rows per station, chosen to match the
  plaque quad's aspect so glyphs engrave square-ish, not as squeezed
  noodles. GLES note: this claims texture unit 3; the kit now expects 4
  units total, comfortably under the GLES 3.0 minimum of 16.
- **Navigation.** `[`/`]` walk prev/next station (wrapping), clicking a
  plaque jumps to it (plaques hit-test before the page), the focused plaque
  burns brighter, and the page snaps past the same 120-line horizon the
  editor's follow camera uses — a thousand-line glide is a blur, not a
  journey. `max_top` clamps the snap so the page never scrolls past EOF
  (load-bearing: a test that asserted the unclamped value failed correctly).

Honest ledger: policy, layout, env derivation, atlas rasterization, and
jump semantics are unit-tested headless (15 in the scene crate). The GL
path — second texture unit, corridor rendering, flight FEEL (rates 5.0
target / 2.2 env-mix) — is compiled-only: no display in the build sandbox.
Visual verification and tuning is step one on a real machine.

## Revision preview in the flat editor (Ctrl+, / Ctrl+.)

The flat shell can time-travel through the same merged git+gitmind
timeline aperture-view walks physically — same index, same "past the
newest comes home" rule, so "revision 3" means the same thing in both
shells. Entering preview is READ-ONLY, enforced at the event-dispatch
chokepoint (the same `handle_preview`/`handle_find` idiom): typing,
paste, and cut are swallowed rather than silently no-oping, while cursor
motion, scrolling, and further `,`/`.` navigation pass through normally —
browsing a fixed text is exactly what the editor's motion code already
does. Escape returns home in one gesture regardless of how deep you've
gone; the pane also grows an amber border so the read-only state doesn't
depend on noticing the status chip.

**Honest tradeoff, stated plainly rather than hidden**: the live buffer's
TEXT is snapshotted and restored exactly (an unsaved edit survives a
preview round trip byte-for-byte — that part is load-bearing and tested),
but the undo STACK does not survive the round trip. `Editor`'s history
isn't cloneable, and cloning it just to support a read-only side trip felt
like the wrong invasiveness to add to aperture-core. If that turns out to
matter in practice, the fix is giving `History` a `Clone` impl — a small,
contained change — rather than anything about this feature's design.

## Brace-split Enter — another TODO.md item, closed, and a real bug it surfaced

`Editor::newline_smart` (`aperture-core`, headless, no shell dependency):
caret sitting directly between an adjacent `{}` — exactly auto-pairs' own
output — splits into three lines instead of one plain newline: the opener,
an empty line one indent level deeper with the caret, the closer pulled
back to the OPENER's indent. One `insert_text` call, so undo removes the
whole split as a unit, not line by line. `insert_text` naturally lands the
caret at the END of what it inserted (after the closer); the caret's real
destination — the middle line — is set by hand afterward with a plain
`Selection::caret`, since `Selection`'s fields are already public and nothing
about this needs new API surface. Anything else (not a caret, or a caret not
between an adjacent pair) falls straight through to the existing `newline`.

**A second real bug, found by re-reading this exact code path rather than
introduced by it**: revision preview's read-only swallow list — the same
`handle_preview` gate `Ctrl+B`/`Ctrl+M` depend on — never included `Enter`.
It's been possible to press Enter while "previewing" a revision and have it
silently call `editor.newline` (now `newline_smart`), genuinely mutating a
buffer the status chip and the amber border both claim is read-only. Fixed
by adding `Enter` to the same swallow arm as `Backspace`/`Delete`/`Tab`, with
a test that asserts the buffer is byte-for-byte unchanged after pressing
Enter mid-preview — a test that would have failed against the code as it
stood five minutes earlier.

## Fuzzy file picker for Ctrl+O — the TODO.md-named next step, done

`file_finder.rs` walks the workspace root once per open (`std::fs`, no new
dependency — explicit stack, not recursion, so a deeply nested tree can't
blow it even past the directories it already skips by name) and fuzzy-ranks
the result against the typed query with the same `uikit_egui::fuzzy` scorer
the command palette already uses, so Ctrl+O feels like the rest of the
suite's pickers. `workspace_root()` prefers the git repo root (reusing
`gitspace::discover`, already done at load) over the open file's own
directory — open a file two levels deep and the picker still finds
everything else in the tree, not just its siblings.

Typed-path entry was never removed, only demoted to the fallback: when
nothing fuzzy-matches (a path outside the walked root or a repo past the
walk cap), Enter still opens exactly what was typed. That was the original
design's whole justification ("honest, works over ssh") — the fuzzy layer
is additive, not a replacement for the escape hatch that still matters on
a phone over a slow connection.

## Touched stations: which rooms did THIS diff actually change

The corridor already showed history's residue (heat, from blame) but never
answered the sharper question a diff poses: which of these specific rooms
did the change on screen touch? `Station.touched` closes that — a station
is touched when the diff currently shown (revision preview, or the live
worktree's uncommitted lines — whichever `self.changed` holds) overlaps
its item's line span at all.

Deliberately a DIFFERENT color, not a brighter shade of heat's ember tint:
git-green plinth plus a small emissive marker cap, visible before the
color read is even legible from down the corridor. Heat answers "what's
been busy lately" (the item's whole history); touched answers "did this
one change reach here" (the diff on screen, right now) — conflating them
into one signal would have answered neither question well.

`rooms::layout_full` is the one function that knows both blame ages and
diff ranges; `layout`/`layout_with_ages` are thin callers into it with
empty diffs, so every existing call site (including the unit tests) kept
its exact old signature and behavior.

## Materialize (Ctrl+M / M) — gitmind's other named checkpoint move

gitmind's checkpoint.rs `materialize` — `git worktree add --detach` — is
the other half of "aggressive branching with checkpoints." Where
`branch_from` says "keep developing from here," `materialize` says "stand
in this version, in a real directory, without disturbing the one I'm in":
open it in another editor, run its tests, diff two whole trees on disk.
`suggest_worktree_path` places the new directory as a SIBLING of the repo
root (`../myproj@add-the-thing-cafef00d`), never nested inside the tree
it's a snapshot of — no gitignore entry needed, no risk of the checkout
polluting `git status` in the repo you're standing in.

**A real bug this caught, worth stating plainly rather than glossing
over**: `branch_from_here`'s live-editor guard ("Ctrl+B does nothing but
show a message while live") was ONLY wired inside `handle_preview`'s gate
— which by definition never runs while `rev_ix` is `None`. So Ctrl+B on
the live file wasn't guarded at all; it was a true, silent no-op, key
event dropped on the floor, no message, nothing. My original test for
that feature only asserted the NEGATIVE ("no branch appears") — which
passes whether the guard fires correctly or the key is silently ignored,
so it never caught the gap. Writing `materialize_here`'s test the same
way surfaced the exact same class of bug immediately (`unwrap()` on a
`None` status), which is what prompted going back and tightening BOTH
tests to assert the guard message is actually present, not just that
nothing destructive happened — and fixing the actual missing key
bindings in the live dispatcher for both `B` and `M`.

## Branch from here reaches the corridor too (the parity gap named last time)

`branch_from_here` is now on `Viewer` as well — same guard (a no-op with a
message while live, a real branch while previewing a station's revision),
same `gitspace::branch_from` / `suggest_branch_name` underneath. One
difference, forced by the shells' different clocks: the flat editor times
its status chip off `now_ms` wall-clock; the viewer has no such clock, only
`step(dt)`'s per-frame delta, so the result message (`branch_msg`) is a
`(String, f32)` countdown that decays with the same dt everything else
animates on, rather than an expiry timestamp. Key is plain `B` (no Ctrl —
aperture-view's binding surface doesn't reserve modifier chords the way the
editor's does) and takes chrome priority over the assistant's margin note
while it's alive; they're never about the same thing at once.

## Branch from here (Ctrl+B, previewing only) — gitmind's own pattern

gitmind's ARCHITECTURE.md names this as a first-class move: "aggressive
branching with checkpoints — promoting any checkpoint to a real branch is
one `branch <n> <ckpt-oid>`." `gitspace::branch_from` is exactly that
operation, reachable from wherever you're LOOKING — the flat editor's
preview mode today, since that's where an oid is already on screen (the
physical corridor is the natural next place for the same command, not yet
wired). `suggest_branch_name` turns a revision into a readable, fully
DETERMINISTIC name (`from/<subject-slug>-<oid8>`) — same revision, same
suggestion every time, so retrying after a collision shows the identical
name rather than a fresh one.

**The footgun this was built to avoid**: `Ctrl+B` on the LIVE file does
nothing (with a status message saying so) rather than branching HEAD by
accident — branching is only reachable through `handle_preview`'s gate,
which by construction only runs while `rev_ix.is_some()`. Collisions pass
git's own refusal message through unchanged rather than re-wording it
("fatal: A branch named '<name>' already exists." is already exactly
right). Tested hermetically: no branch appears while live, a real branch
appears with the expected name while previewing, and a repeat attempt
surfaces the collision in the status chip.

## The assistant's margin note reaches the flat editor too

Revision preview in the flat shell now carries the same ✎ margin note
aperture-view shows: landing on a revision fires a request to a DEDICATED
`describe` engine (own `APERTURE_CMD` provider, independent of the
ghost-completion engine — a "describe this diff" prompt and a
fill-in-middle prompt are different jobs even when they share a provider
binary), one sentence on what changed and when you'd want this version.
Newest-request-wins means scrubbing through history describes only
wherever you settle; going home clears the note. No `APERTURE_CMD`, no
engine, no cost — checked directly (constructing `AppState` in a bare
test process, without setting process-global env, naturally exercises
this path).

Tested end to end against a REAL subprocess (`echo`, not a mock): the
request fires on landing, the note populates within the engine's actual
thread-hop-and-poll latency, and it clears on returning home.

## gitspace lives in aperture-core, and the flat editor uses it too

`gitspace` moved from aperture-scene into aperture-core: it has zero
scene-specific dependencies (`std::process::Command`, nothing else), and
duplicating its parsers for the flat shell — or making the text editor
depend on scene-kit/eframe to ask git a question — were both worse than
sharing. The 2D editor gets the same "diff as marginalia" idea in its own
vocabulary: a green gutter bar for lines uncommitted vs. the newest commit,
discovered on open and refreshed on save (not per-keystroke — no
diff-per-edit churn), plus a status chip totaling the changed count.

## gitspace: history the corridor can wear

The gitmind integration is at the PROTOCOL level, not the crate level.
gitmind-core's `GitCli` philosophy — shell out to `git`: full ecosystem
compat, ~5–20 ms/query, no libgit2 build burden — is adopted wholesale, and
aperture-view reads gitmind's checkpoint namespace `refs/checkpoints/*`,
the shadow-commit timeline its autocommit engine writes without touching
HEAD. gitmind writes the timeline; aperture-view walks it in space.
(Path-depending on gitmind-core was rejected: its chunk store — redb,
zstd, fastcdc, notify — is not feature-gated, a heavy payload for a viewer
needing four subcommands. If gating lands, the `Runner` trait is exactly
the seam where `GitCli` slots in.) Every parser is pure and fixture-tested;
one hermetic test builds a real throwaway repo. A same-second-commits bug
taught the merge to be a STABLE time sort — log order is the authoritative
parent order and must survive rapid autocommits.

What history looks like, physically:

- **Heat.** `git blame --line-porcelain` → per-line last-touch times → each
  station's heat = MAX over its span (one hot line makes a warm room —
  "what changed lately" is the wayfinding question). Half-life a week.
  Fresh masonry glows ember and emissive; ancient stone stays cool. Blame
  runs for the live worktree only (historical revisions would need
  blame-at-rev; deferred, those stations stay cold).
- **Revision paging.** `,` older / `.` newer; newer past the newest comes
  home to the live worktree. Swapping content REBUILDS the rooms — the
  corridor IS that version's file, stations appear and vanish across time;
  you watch the architecture grow. Checkpoints wear ◈ in the status line.
  Missing-file revisions (checkpoints predating it) are stepped over.
- **Diff as marginalia.** Changed lines wear a green-inked gutter, the way
  a proofreader marks a manuscript: viewing a revision marks what IT
  changed vs its parent; viewing the worktree marks what's uncommitted vs
  the newest commit.
- **The assistant's margin note.** The SAME engine + env contract as the
  editor (`APERTURE_CMD`, whitespace-split argv): stepping onto a revision
  asks the provider for one sentence — what this change did, when you'd
  want this version — shown as ✎ in the chrome. The newest-request-wins
  worker is exactly right for scrubbing: flick through ten revisions and
  only the one you settle on gets described; the rest coalesce away
  unsent. No provider env, no note, zero cost.

Honest ledger: parsers, merge, heat, paging round-trip (hermetic repos),
and the zero-cost no-provider path are tested; the describe path with a
LIVE provider is compiled-only (engine behavior itself is tested in
aperture-complete), and all rendering remains unverified visually.

## Follow camera (Ctrl+E — impakt-cam, distilled)

The user's Atom plugin `impakt-cam` supplied the spec: the viewport doesn't
merely keep the caret visible, it *glides to keep it centered* — a game
camera for code. The port keeps its three load-bearing knobs and drops one
on purpose:

- **Dead zone** (`lineCenterThresh` → 2 lines, in TOP space not caret
  space): typing in place doesn't move the world, and the file's edges —
  where centering clamps — go quiet for free instead of fighting the clamp.
- **Snap horizon** (`maxLinesToScroll` → 120 lines): past it, teleport. A
  5000-line jump animated is a sickening blur and a frame-cost cliff.
- **Distance-scaled speed** (`rowModifier`): free. The Smooth's rate×error
  physics already covers longer glides at proportionally higher px/s.
- **Elastic ease: not ported.** Overshoot-free is a *tested property* of the
  house camera; a viewport that springs past the target line re-reads as
  jank. This is the one place the port disagrees with the original.

Event model, also impakt-cam's: follow applies only when the caret LINE
changes (never mid-mouse-drag), so wheel-scrolling is free browsing until
the next caret motion snaps the leash. The policy (`camera::Follow`) is a
pure function — dead-zone/glide/snap and both edge clamps are unit-tested;
the view's application is six lines. Toggle: Ctrl+E, the palette, a status
chip.

## Find and auto-pairs (the daily-driver gap)

**Ctrl+F** is a modal bar living entirely in `state.rs`: typing goes to the
query, Enter/F3 and Shift+Enter/F3 cycle (wrapping), Esc closes leaving the
match selected (jump-then-edit is the workflow — typing then replaces it),
opening with a single-line selection seeds the query. Case-smart in the
ASCII sense: any uppercase letter → sensitive. That "ASCII" is a decision,
not a shortcut: Unicode case folding changes byte lengths, and a find bar
that misdraws its own highlight rectangles is worse than one that is
case-exact for non-ASCII. Matches recompute behind a (version, query) gate;
the view draws every match as a faint wash and the current one loud.

**Auto-pairs**, three moves, all ordinary history: opener with a selection
*wraps* it (selection kept, one undo); opener at a caret inserts the pair
with the caret between (one gesture — the pair leaves the way it came);
typing a closer already under the caret *skips over* unconditionally
(predictable beats bookkeeping about who inserted it). Quotes never pair
against a word, so `don't` and Rust lifetimes don't fight the editor.
Backspace inside an empty pair eats both.

## aperture-view: the physical viewer (second shell, same core)

The second deliverable of the session: *view files with logscope's physical
UI and scenes.* It exists to prove the layering — `aperture-core` didn't
change by one line to gain a 3D shell.

**The extraction decision.** logscope's physical UI (spatial → mesh3d →
gpuscene → glow_scene, + texgen and the textsurface decision layer, ~3.9k
lines, self-contained) became `shared/scene-kit`, because aperture is its
second consumer and the repo lifts kits at two call sites. logscope keeps
its copy untouched this session and migrates at the next scene co-change
(TODO.md §scene-kit) — the same adapt-then-merge route the ollama client
took. The rejected alternative was depending on `logscope-egui` wholesale:
123 modules of an LLM app compiled into an editor, for four files.

**What the kit gained at extraction** — the two gaps logscope's own
`docs/physical-ui-findings.md` names as blocking the whole direction:

1. **Per-vertex UVs** (12 → 14 floats, `location=5 in vec2 a_uv`).
   `Mesh3.uvs: Option<Vec<[f32;2]>>` — `None` on every existing builder
   packs (0,0) and changes nothing. `MAT_INSCRIPTION` (= 5.0, past the last
   texgen layer) samples a new `u_page` texture *in page space* — so words
   sit ON a face instead of floating. Relief for inscription faces falls
   back to paper (layer 0): parchment tooth under the ink, not the clamped
   moss layer. This was the findings doc's "enabling change", verbatim.
2. **`u_touch` is live.** The proximity helper existed, was uploaded every
   frame, and was never once set — dead code with a silent fallback. The
   kit's fragment shader now drives emissive from it (every surface warms
   faintly, inscribed ink more), and aperture-view sets it: click the page,
   the page answers, age advanced by frame delta so the fade paces the same
   at 60 and 120 Hz — the findings' own prescription.

**The scroll trick.** Rasterizing 42 rows for a 40-row window and sliding
the quad's UV window (`set_page_window` rewrites four UV pairs) means the
`Smooth`-animated scroll costs *zero* rasterization — the page re-inks only
when the integer window moves (`ensure_page`, version-gated; the GL side
re-uploads on version bump alone). Same motion primitive as the editor.

**The scene.** A stone floor, a wooden lectern carrying the inscribed
parchment (overlay-biased so props can never occlude the reading face —
mesh3d's physical-UI depth pull, used as intended), and four plinths behind:
paper, wood, stone, moss — RUN_ON_DEVICE's context-panel materials,
standing where the reader can see what the world is made of.

**Viewer verification ledger.** Headless-tested: viewer brain (7 — raster
window versioning, clamps, touch aging, UV offset), scene packing (2 —
14-float stride, UV ride-along, window slide), inscribe (3 — glyphs, span
colors, scale), plus the kit's 50 inherited tests updated to the new vertex
(53 total). Compiled + boot-smoked only: the GL path (shader edits, page
upload, callback) — no display in the sandbox, so the scene has *never been
seen*. The shader changes follow the existing material-branch patterns and
the attribute/stride math is mechanical, but visual verification is the
first thing to do on a real machine: `cargo run --bin aperture-view -- 
some_file.rs` (add `--ignore-rust-version` on rustc <1.92).

## Milestones next (each lands independently, seams already cut)

1. tree-sitter behind `LineLexer` (syntax.rs untouched elsewhere).
2. Streaming ghosts (`stream:true`, Provider grows a cancel).
3. File-fuzzy Ctrl+O over a workspace walk + `uikit_egui::fuzzy`.
4. Multi-cursor (`Selection` → `Vec<Selection>`, edits fan out).
5. LSP crate on the same worker/inbox pattern as the completion Engine.
6. Physical viewer: logscope migrates to scene-kit (re-export shims +
   `uvs: None` literal sweep); editing on the lectern (the ghost machine is
   shell-agnostic already); a Runestone theme via `textsurface::for_scene`.
