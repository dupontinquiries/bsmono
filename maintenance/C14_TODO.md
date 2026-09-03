# C14_TODO — session c15unified, 2026-09-02

Next in the file series after `C13_TODO.md`. The numbering is the document's,
not the session's: c14 appended its notes to C13_TODO (§60–92) rather than
starting a file, so this is the first new one since c13. Read C13_TODO first
for anything still open there — in particular §19/§27, which §1 below corrects.

Sections are numbered fresh.

---

## 1. §19/§27 was wrong about the cause: it is not deferred wrap

C13_TODO §19 traced voiceterm's garbled panes to the grid mis-tracking the
cursor through a themed zsh prompt redraw, named the deferred-wrap ("phantom
column") model as the likely defect, and §27 directed the next session to read
konsole's `Screen.cpp` to settle it. No konsole checkout is in the c14unified-025
bundle, so this session built the corpus §27 asked for and ran the existing
parser against it instead.

**The hypothesis does not survive contact.** Eight constructed fixtures put the
cursor at the exact right margin and then do something to it — LF, CR, BS, CUF,
EL, ED, a double-width glyph, and a wrap that scrolls the screen. All eight
agree with tmux, unchanged, against the parser as it was. The eager-wrap model
already in `Grid::put` (park at `cur_col == cols`, wrap on the next write) is
observably equivalent to a pending-wrap flag for every case that could be
constructed.

So `§19` should not be worked as a wrap bug, and §27's konsole reading list is
answering a question that is not the one being asked. What was actually wrong
is a list of missing sequences, in `KNOWN_GAPS` in
`shared/uikit/crates/uikit-term/tests/vt_corpus.rs`, each with what it is for.

This is the third time in this repo's notes that a confidently-stated cause in
a design document turned out to be a hypothesis (CLAUDE.md §5). The note in §19
did say "very likely" — the loss was that §27 then built a reading list on top
of it as though it were settled.

**Marked as: HYPOTHESIS DISPROVED by measurement, 2026-09-02.**

## 2. What closed, and what the corpus still lists

Closed this session (CHA/HPA, VPA, CNL/CPL, ECH, ICH, DCH — commit
`31039857`). CHA is the one that most plausibly explains the field report:
readline redraws an edited line by jumping to an absolute column rather than
counting backspaces, so with it dropped the redraw landed wherever the cursor
was and the line interleaved with itself, which is what §19 describes.

Still listed, roughly in the order I would take them:

1. **SGR 38/48 with `5;n` or `2;r;g;b`.** Not ignored — actively corrupting.
   The sub-parameters fall through to the plain SGR table, so `38;5;33` sets
   foreground 3 and `38;2;255;128;0` ends on a `0` that RESETS the pen. Needs a
   `TermColor::Rgb` variant and `Ansi` widened to mean palette index 0–255;
   `src/snapshot.rs` already has the `rgb:rrggbb` spelling waiting for it.
2. **IL / DL / DECSTBM.** No scrolling region exists at all, so a scroll meant
   for part of the screen moves all of it. Every full-screen program sets one.
3. **DECSC/DECRC (`ESC 7`/`ESC 8`) and SCOSC/SCORC (`CSI s`/`CSI u`).**
4. **RI (`ESC M`).**
5. **The ESC-intermediate leak.** `ESC ( B` prints a `B`, `ESC # 8` prints an
   `8`. Half-implemented rather than absent, which is the worse state.
6. **Double-width filler cell** reads back as a space, so `row_text` returns one
   more column than the screen shows.

## 3. Not done and probably the bigger half of §20: the alternate screen

`CSI ?1049h` / `?1047` / `?47` are unhandled, so there is no alternate screen
buffer. vim, less, htop and anything else full-screen draw over the primary
screen and never restore it. Nothing in the corpus covers this yet because no
fixture spawns such a program.

This is likely a larger part of "voiceterm-egui panes still unusable as a real
terminal" (§20) than everything in §2 combined, and it is its own session: it
wants a second cell buffer, saved cursor/pen across the switch, and a decision
about whether the alternate screen contributes to scrollback (it must not).

## 4. The corpus is bash-only; the reported shell was zsh

`tools/capture_vt.py` records from a real pty, but zsh is not installed in this
sandbox, so the eight recorded scenarios are bash/readline. The original report
was a themed zsh, whose zle emits a different redraw shape (reverse-video
placeholder, run of spaces to the terminal width, `\r`-and-redraw). Capturing
those on a machine with zsh is cheap — add scenarios to `SCENARIOS` and run the
two tools — and is the single highest-value addition to the corpus.

## 5. `lock_check.py` cannot see a lock made stale by a transitive path dep

It reported `stale: 0` on a tree cargo was actively rewriting. It compares a
crate's own `[dependencies]` names against the names in its lock, so when
`uikit-egui` — a path dependency of `uikit-term` — gained `math-kit`,
uikit-term's committed lock went stale in a way the checker cannot express.
Found by tripping over it, which is the same way the original three stale locks
were found (CLAUDE.md §7).

Fix shape: walk `path = ` dependencies transitively from each lockfile's crate
and union their declared names into `want`, rather than reading one manifest.
Cycles are possible in principle and the walk needs a visited set.

Not fixed here; the lock itself was refreshed in `8b483653`.

## 6. `eframe_backend_check` fails on unendlich-dash, and the fix is not obvious

`games/unendlich/crates/unendlich-dash/Cargo.toml` declares
`eframe = { version = "0.34", features = ["glow"] }` with defaults left on.
Confirmed against the published manifest (fetched from static.crates.io):
eframe 0.34's `default` includes `wgpu` and does NOT include `glow`. So the
crate links BOTH backends, one stated and one inherited, which is exactly what
the checker is named for.

**But the source wants both.** `src/main.rs` runs `run(eframe::Renderer::Wgpu)`
and falls back to `run(eframe::Renderer::Glow)`. So this is not over-linking by
accident, and the naive fix — adding `default-features = false` — would leave
`Renderer::Wgpu` with no backend compiled.

Restating the defaults explicitly is the right shape, but it is not a
mechanical edit: eframe's `default` also contains `accesskit`, `default_fonts`,
`wayland`, `x11`, `web_screen_reader` and **`winit/default`**, and that last one
cannot be re-enabled from a dependent crate's feature list. Turning defaults off
would silently drop winit's own defaults with no way to restore them from here.

Pre-existing, not introduced by this session's merges (the file is byte-identical
before and after). Left alone deliberately: `unendlich-dash` is recorded as
TIMEOUT-on-build in C10_TODO §77 and unverified in C11-4c, so there is no way to
test either branch of this in the sandbox, and an unverifiable edit to a feature
set is how a working GUI stops starting.

Worth considering instead: the checker gaining a case for "both backends, chosen
at runtime with a fallback", which is a legitimate pattern it currently has no
way to express.

## 7. Merges taken this session

Four bundles offered, three merged, all with zero file overlap between them and
zero conflicts — four genuinely disjoint work streams.

| bundle | branch | result |
|---|---|---|
| `c14unified-001-rusty-board-v2-flick-swipes` | `inc-rustyboard` | merged, 8 commits |
| `c14unified-009-fao-weighted-pred` | `inc-fao` | merged, 14 commits |
| `unendlich-perf-work--c14unified-022-delta` | `inc-unendlich` | merged, 8 commits |
| `blackstar-decision-to-change-engines--c14unified-010-delta` | `inc-blackstar` | **already contained in main** — nothing to merge |

The blackstar delta's tip `2f97001c` is an ancestor of main, so its sixteen
commits were already here. The accompanying `engine-change.txt` is that
session's write-up and covers blackstar-rs gameplay, the game/engine split, and
the unendlich work that arrived separately as `-022`.

**Verified for the merges: they apply clean and the checkers above run.** Not
verified: none of the merged code was built. `games/unendlich` is a wgpu target
with no `/dev/dri` here, `apps/fao-codec`'s numbers were not reproduced and its
suite (reported 102/0) was not re-run, and rusty-board's wayland/GTK and KWin
script paths are not buildable in this sandbox. Each merge commit says so in
its own message.

## 8. dandere2x arrived as a SECOND LINEAGE, not a delta — decide before merging

`aaaa_tar.zst` carried seven bundles. Six merged (four clean, one with a
resolvable conflict, and two that were ancestors of a third). The seventh,
`dandere2x-since-59e520e3.bundle`, is not mergeable as it stands and the
reason is worth writing down rather than rediscovering.

**All 17 conflicts are add/add.** `apps/dandere2x` does not exist at the merge
base (`59e520e36`). Both sides created the whole app from nothing, and every
one of the bundle's nine commits collides with files main already has —
18 of 19 in the first commit, 12 of 12 in the second. There is no additive
subset to harvest; the two lineages built the same files independently.

### What each side has that the other does not

| | main | bundle |
|---|---|---|
| commits | 6 | 9 |
| .rs files | 17 | 22 |
| lines | 6,073 | 7,099 |
| `#[test]` | 61 | 57 |

**Bundle-only:** `d2x-job` as its own crate (ffmpeg, job), plus `batch.rs`,
`blockcache.rs`, `budget.rs`, a `bench_hot` test, and a checked-in
upconv_7 model. Its commits claim a generic conv network (RRDBNet/SRVGG as op
lists), a torch-free pth2st converter, GPU bit-exactness against CPU on
lavapipe for three networks, keyframe-aligned segment parallelism, a difficulty
cascade, memory budgets, a content-keyed block cache, and a hot-path audit
tested bit-identical at 2/3/5/8 threads.

**Main-only:** `crates/dandere2x/src/{ffmpeg,job}.rs` — the same two concerns
the bundle promoted into `d2x-job` — plus the SEAM WORK. `feather`,
`overlapped` and `tile_pad` appear in main and in NO bundle file.

### Why that last row is the decision, not a detail

C13_TODO §84 corrected the seam diagnosis twice-over: the flickering seams are
the residual bleed ring, not the CNN tile grid. Both lineages found the bleed
ring — the bundle names it in three files, main in one — so that much converges.

But §84 also says, explicitly, that the CNN-tiling work is **not** wasted:
"`pre_pad` 8->32 and the overlapped blending are real improvements to a real
(if less visible) artefact, and they are independently tested." Taking the
bundle wholesale drops that, and drops it silently, because nothing in the
bundle references it to conflict with.

So neither side is a superset. The bundle is broader in scope; main carries a
tested fix for a second, real artefact that the bundle never addressed.

### What I did not do, and why

I did not resolve 17 add/add conflicts by hand. Every file is GPU or codec
code whose correctness claims are bit-exactness against hardware this sandbox
does not have — no `/dev/dri`, lavapipe only — so a hand-merge would be
seventeen judgement calls I could not test even once. Picking a winner per file
on reading alone is how two working implementations become one that compiles.

The branch is fetched and kept as `inc3-dandere-work/main`; nothing is lost.

### The three ways forward, cheapest first

1. **Bundle wins the directory, seam work re-applied on top.** Four commits to
   port (`3aab45291`, `5ff55cf53`, `030a03100`, `ada7dbfaa`), all small and all
   about padding and blending, against a `d2x-core` that has moved under them.
   Recommended if the bundle's author has been the one running this on real
   footage.
2. **Main wins, bundle harvested feature by feature.** More work — `d2x-job`,
   the cascade, budgets and the block cache are each substantial — but it keeps
   the tested seam behaviour without a port.
3. **Ask which line has actually been RUN.** Both make bit-exactness claims
   neither can be checked here. The one that has upscaled real video recently
   is the trunk, and that is a question about history rather than about code.

## 9. `unendlich-world` does not compile, and nothing was watching that binary

Found while verifying the shader-path conversion against the operator's own
build line. `cargo check -p unendlich-engine --features render,window,ui-overlay`
reports **12 errors, all in `src/bin/unendlich_world.rs`** — a declared `[[bin]]`
(`unendlich-world`) that nobody builds.

They are not from this session's work. The shapes are API drift:

* `input.handle_event(&event)` given a `&WindowEvent` where `&InputEvent` is
  expected
* `is_key_just_pressed(Key::Escape)` where a `Keycode` is expected

That is `input.rs` having moved to its own event and key types while this
binary stayed on winit's. Nothing in the conversion touched either.

**The operator's target is clean**: `cargo check --bin unendlich-fullgame
--features render,window,ui-overlay` — 0 errors, which is what confirms
6420767ee fixed the reported `PathBuf`/`String` mismatch.

### Why it is worth an entry rather than a fix

Two things are unclear and both change the answer:

1. Whether `unendlich-world` is still wanted. It may be a superseded shell that
   `unendlich-fullgame` replaced, in which case CLAUDE.md §7b applies — archive
   it with provenance, do not repair it.
2. Whether the drift is one adapter or twelve separate ports. Eleven of the
   twelve errors are in that one file, which suggests an adapter, but "suggests"
   is not a reading.

Fixing a binary nobody builds, without knowing whether it should exist, is how
dead code acquires tests and stops being removable.

### The general point, which cost this session a bad claim

I checked `-p unendlich-engine --lib`, saw it clean, and reported the shader
conversion verified. The default features do not compile the sparse mesher, so
the one real error in my own change reached the operator instead of me.
CLAUDE.md §3 already says this — "check the NON-default configurations before
believing a crate is green", with clawbound as the precedent — and the entry
exists because knowing the rule was not enough.

**A crate with feature-gated modules is not verified by its default check.** For
this crate specifically the line is
`--features render,window,ui-overlay`, and the two useful invocations are the
whole package (which surfaces §9) and `--bin unendlich-fullgame` (which is what
ships).

## 10. The recurring defect this session found six times: ported, not wired

Six separate instances, in six different apps, of the same shape — code that
exists, compiles, is often tested, and **nothing calls it**. Listing them
together because individually each looked like a one-off and collectively they
are a pattern with a cheap detector.

| what | how it presented | found by |
|---|---|---|
| `uikit_egui::fonts::install` | eleven apps drew boxes for glyphs that worked elsewhere | reading switcheroo's colours for an unrelated reason |
| `d2x-job::preflight` | ported back, then had no caller for an hour | grepping for callers before claiming it done |
| `net::Blender` | 249 lines and its own tests; `write_output_rgba8` still hard-cropped | the commit that ported it said so |
| `camera::viewfinder_height` | helper with tests, no callers, after its call site changed | noticing the tests were about nothing |
| snapclone's shutter | fully wired; the FEEDBACK was a text line nobody sees | tracing it end to end instead of believing the report |
| `crates/dandere2x/src/{ffmpeg,job}.rs` | 861 lines, 5 tests, undeclared in `src/` after a merge | `orphan_check.py` |

Only the last was caught by a tool. `orphan_check.py` works at FILE
granularity — a file with no `mod` reaching it — and every other row here is a
function or a call site, which it cannot see.

### The detector that would work, and why I did not write it

"Public item with no non-test caller anywhere in the workspace" catches all six.
It also fires on every genuinely-public API — `uikit-egui` exists to be called
from outside itself — so it needs a notion of which crates are libraries for
other crates and which are app internals, and that distinction is not in
`manifest/projects.toml` today (`role = "lib"` exists but only three crates set
it).

Two ways in, cheapest first:

1. **App crates only.** Skip anything under `shared/`, and anything with
   `role = "lib"`. Within an app, a `pub fn` with no caller is nearly always
   dead or unwired, because nothing outside links it. That would have caught
   `preflight`, `viewfinder_height` and `Blender`.
2. **Whole fleet, with a KNOWN list**, the way `font_install_check.py` handles
   its eight. More work, more value, and the list is the interesting output.

Not written because a checker that cries wolf gets ignored (`lock_check.py`'s
first run said 26 and three of those classes were false), and calibrating this
one needs a pass over the results that I did not have room for.

### The habit that actually caught them

Five of the six were found by asking "what calls this?" immediately after
writing or merging something — not by a tool. That question belongs in the
routine next to the build, and it is cheaper than the checker:

```
git grep -n 'the_new_function' -- '*.rs' | grep -v test
```

If the only hits are the definition and its tests, it is not done.

## 11. Verification traps hit this session, as commands rather than principles

Three, all the same species: the obvious invocation was not the one that
matters.

* **`cargo check -p unendlich-engine --lib` is not the build.** Default features
  skip the sparse mesher, so a real type error in my own change reached the
  operator. The line is
  `cargo check --bin unendlich-fullgame --features render,window,ui-overlay`.
* **`cargo check --all-features` is not the build either.** On `beatspace` it
  pulls a macOS-only path and fails on `objc2` before reaching any of the
  crate's own code. Default features are clean.
* **A break test needs its backup from git, not from disk — and its RESTORE
  must be as narrow as its edit.** A helper that restored with
  `git checkout -- shared/.../src/` reverted `lib.rs` along with the file it
  meant to fix, removing a `pub mod` declaration. The module stopped compiling,
  its tests silently stopped running, and a break that should have failed three
  suites reported zero. A restore that touches more than the one file the break
  touched can turn a real failure into a clean bill. Twice a batched
  break loop failed partway, and the next command's `cp` captured the ALREADY
  BROKEN file as the "clean" backup; the restore then put back a file missing a
  guard, and only a test caught it. `git show HEAD:path > /tmp/backup` has no
  such failure mode.

## 12. unendlich touch: the model and the wiring are done, the BACKEND is the blocker

Three commits landed a working touch path: `touch_controls` (the model, 11
tests), `InputCache` integration (5 tests, and it found a real bug — see
below), and `touch_event_from_winit`.

**None of it can receive an event today, and the reason is not fixable in this
crate.** From `unendlich-engine/Cargo.toml`, written by whoever added the winit
dep:

> winit gives real per-finger multi-touch (WindowEvent::Touch with id+phase),
> which GLFW cannot report on Linux at all — it only ever sees a single
> emulated pointer from the compositor.

The shipping backend is glfw. `window-winit` exists, compiles, and is "NOT
wired into unendlich_fullgame.rs yet, additive only". So on a phone the game
receives no touch events whatsoever, and no further work on the model changes
that.

### What is left, in order

1. **Wire `window-winit` into `unendlich_fullgame`.** The converters exist for
   keys, mouse and now touch; what does not exist is a winit event loop in the
   binary. This is the whole remaining job and it is a window-management change,
   not an input one.
2. **Call `tick_touch(dt, width)` once at the top of the frame.** One line,
   but it must be before consumers read — `end_frame` clears `cursor_delta`, so
   folding late zeroes the drag on the frame it arrived.
3. **Read `move_axes()` in the player controller** and blend it with the WASD
   axes rather than replacing them, so a keyboard still works on a device with
   both.
4. **Draw something.** The controls are invisible: no stick ring, no hint. That
   is defensible for look (nobody draws a look zone) and not for the stick,
   where a first-time player has no way to discover the left half is one.

### The bug the wiring found, recorded because the shape recurs

`InputCache::default()` starts with screen width 0, and a tap on the first
frame beats the first resize. Role was computed eagerly at touch-down, so that
tap classified against width 0 — `x < 0.0` is false for every finger — and the
player's FIRST thumb-down was always a look, never the stick.

It would have been reported as "the movement stick doesn't work until you touch
the screen twice". Nobody maps that back to a width of zero.

The general shape: **a pure model can be correct in isolation and wrong in the
only configuration that ships.** The model's own eleven tests all constructed
`TouchState::new(1000.0)`, so none of them could see it. Writing the caller
found it in one run, which is an argument for wiring promptly rather than
banking a tested model — and a counterweight to §10, which is about the
opposite failure.
