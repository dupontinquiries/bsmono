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

**CORRECTION, 2026-09-04: everything in this section closed the day after
it was written.** Commits `0af1ff97` (extended SGR 38/48), `7db662d0`
(scrolling region — DECSTBM, IL/DL, SU/SD, RI), `6707d23d` (save/restore,
ESC intermediates, DECALN, wide-glyph tail) and `2accbb2e` (the alternate
screen, §3 below) all landed 2026-09-03. `KNOWN_GAPS` in
`uikit-term/tests/vt_corpus.rs` is empty, and `the_whole_corpus_is_enforced`
passes on the full fixture set — confirmed by running the suite, not by
reading the commit log. A session that trusted this section as still-open
would redo finished work. Left the original text below for the record,
per this file's own §1 about compounding-hypothesis debt.

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

**CORRECTION, 2026-09-04: also closed, in `2accbb2e` — see §2's note.**
vim/less/htop now get a real alternate-screen buffer; confirmed live
under Xvfb (`ls --color` and long-line wrapping render correctly in the
actual `voiceterm-egui` binary, not just in the corpus test). Original
text below for the record.

`CSI ?1049h` / `?1047` / `?47` are unhandled, so there is no alternate screen
buffer. vim, less, htop and anything else full-screen draw over the primary
screen and never restore it. Nothing in the corpus covers this yet because no
fixture spawns such a program.

This is likely a larger part of "voiceterm-egui panes still unusable as a real
terminal" (§20) than everything in §2 combined, and it is its own session: it
wants a second cell buffer, saved cursor/pen across the switch, and a decision
about whether the alternate screen contributes to scrollback (it must not).

## 4. The corpus is bash-only; the reported shell was zsh — TOOL DONE, BLOCKED ON TMUX

**Progress in `d7e7934b`, not closure.** `capture_vt.py` now supports zsh
(it needed different plumbing than bash — no `--rcfile` flag, so the rc lives
in a `ZDOTDIR`-pointed directory as a file literally named `.zshrc`) and has
four zsh scenarios ready, including `zsh_eol_mark`: zsh's default `PROMPT_SP`
marks output with no trailing newline via a reverse-video placeholder, pads
to terminal width, then `\r`-and-redraws — exactly the "themed
zsh...different redraw shape" this entry describes, and something bash has
no equivalent of at all.

Caught a real bug in my own first draft before it shipped: I wrote
`unsetopt PROMPT_SP` believing it was framing around the mark: it's the
option that PRODUCES the mark, and with it off the mark never appeared —
found by diffing captured bytes with the option on vs off, not by trusting
the docs.

**Blocked**: `tmux new-session` reports "server exited unexpectedly" in this
sandbox before even binding a socket — not a permissions issue (the socket
dir gets created with correct perms), reproduced across several environments
and with verbose logging that never even opens its own log file. `pyte`
alone is not trusted for `.expect` generation (see `render_ref.py`'s own
docstring — `save_restore` is the worked example of pyte silently encoding a
bug as correct), so no `.bin`/`.expect` pair was committed. The captured
bytes were verified by hand against the described defect shape and are very
likely trustworthy — the pty mechanics are identical to the unchanged bash
path, confirmed byte-identical to its own checked-in fixture after the
refactor — but that is not the same as tmux confirming them.

**What unblocks this**: `tools/capture_vt.py zsh_eol_mark zsh_prompt_echo
zsh_edit_in_place zsh_wrap_long_line` then `tools/render_ref.py`, on a
machine where tmux actually starts.

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

## 6. `eframe_backend_check` fails on unendlich-dash — RESOLVED, see below

**Resolved in `2a6b6563`.** The checker now recognises the dual-backend
runtime-fallback pattern (both `Renderer::Wgpu` and `Renderer::Glow` named
explicitly in source) as a legitimate reason to leave `default-features` on,
rather than flagging it as accidental over-linking. Full rationale in the
checker's own docstring; the analysis below is left as the record of how
that shape was found.

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
at runtime with a fallback" — done, see `2a6b6563` at the top of this entry.

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

## 8. bscale arrived as a SECOND LINEAGE, not a delta — decide before merging

`aaaa_tar.zst` carried seven bundles. Six merged (four clean, one with a
resolvable conflict, and two that were ancestors of a third). The seventh,
`bscale-since-59e520e3.bundle`, is not mergeable as it stands and the
reason is worth writing down rather than rediscovering.

**All 17 conflicts are add/add.** `apps/bscale` does not exist at the merge
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

**Main-only:** `crates/bscale/src/{ffmpeg,job}.rs` — the same two concerns
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
| `crates/bscale/src/{ffmpeg,job}.rs` | 861 lines, 5 tests, undeclared in `src/` after a merge | `orphan_check.py` |

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

## 13. snapclone sends photos as UNCOMPRESSED RGB, and copies them per recipient

Two separate costs on the same path, found reading `HostCommand::SendSnap` in
`snapclone_egui.rs`. Neither is a bug in the sense of something being broken —
the send path is complete and careful, with NAT punching, address health,
real sequence numbers and per-peer sessions. The problem is the payload.

### The wire format is PPM

`frame_pump::ppm_bytes` is a 12-byte header over raw RGB. The comment on the
capture arm explains the choice and it is a good one FOR DISK: "no encoder
dependency, and every image tool opens it", explicitly "not the composer flow
yet — that is its own thread of work".

But the same bytes go on the wire, and there the arithmetic is different:

| frame | on the wire |
|---|---|
| 720p | 2.8 MB |
| 1080p | 6.2 MB |
| 12 MP phone camera | ~36 MB |

A JPEG of the same 1080p frame is 200–400 KB. That is a 15–30× difference on a
cellular link, for an app whose entire purpose is sending a photo to someone.

The one property worth preserving is already stated in the code and is right:
the bytes kept for Send are the same bytes written to disk, "so what a peer
receives is byte for byte what the local file holds. A second encoding path is
a second thing that can differ subtly from the photo you looked at." So the fix
is to change the ONE encode, not to add a second — the disk format and the wire
format should move together.

Cheapest route, in order:
1. **Compress the PPM**, no encoder and no format change — zstd is already in
   the workspace (photosync). Lossless, maybe 2-3× on photographic RGB. Least
   value, least risk, and does not change what a `.ppm` on disk is.
2. **JPEG via the `image` crate**, already a dependency of photosync and
   bscale. 15-30×, and `.jpg` is what a user expects a photo to be. Changes
   the camera roll's file extension, so `save_ppm` and anything reading the
   directory move with it.

### The payload is cloned per recipient

`SnapFrame::announce` takes `payload: Vec<u8>` and `SnapFrame.payload` owns it,
so a group send of five allocates five full copies and hands one to each
sending thread. At 1080p that is 31 MB in flight for one press of a shutter;
at 12 MP it is 180 MB.

`Arc<[u8]>` on the payload fixes it and the change is structural rather than
local — `snap_wire`, `snap_transport` and the tests all move together. Worth
doing WITH the compression change rather than before it, because a 300 KB
payload copied five times is 1.5 MB and nobody would notice.

### Not started

Both are real and neither is a two-line fix. Recorded with the numbers so the
next session does not have to re-derive them, and so the ordering argument —
compress first, then share the buffer — does not have to be rediscovered.

## 14. snapclone voice: the Opus wrapper exists, compiles, and nothing links it

The same shape as §13 one layer down, and further along than that one — because
here the replacement is already written.

### What ships today

`snapclone_egui.rs` sends **raw S16LE**, and says so plainly:

> RAW S16LE, NOT OPUS. There is no encoder linked here, and the honest way to
> say that is to send the samples as they are rather than to claim a codec. It
> costs ~32 kB/s per peer against Opus's ~2, which is fine on a LAN and is not
> fine on mobile data — swapping in a real encoder is a change to these two
> conversions and nothing else, which is why they are the only place bytes and
> samples meet.

That is a good comment and an honest one. 16x on a cellular link, and in a
four-person call it is ~96 kB/s up rather than ~6.

### The encoder is already there

`apps/snapclone/rust/snapclone-audio/src/codec.rs` is a complete `audiopus`
wrapper — encoder, decoder, packet-loss concealment on a NULL packet, bitrate
and bandwidth control. Feature-gated `opus`, off by default, with its own note
that it "is validated on the device build, not in the headless test sandbox".

**It builds and its crate's tests pass.** `libopus-dev` installs from apt here
in one command; `cargo check --features opus` is clean and
`cargo test --features opus` is 104 green. So the "validated only on device"
caveat is now out of date: it validates here too, and that should be recorded
in the module rather than left as a reason not to try.

`snapclone-egui` does not depend on `snapclone-audio` at all. The crate that
needs the encoder does not link the crate that has it.

### The dead warning

`voice_send::Emit::NoEncoder` and `sender.note_no_encoder()` exist to tell the
user they are transmitting without a codec, and `note_no_encoder` is carefully
written to report ONCE rather than at fifty hertz. The binary calls
`EncoderReady(true)` unconditionally at line 653 — while sending raw PCM — so
the warning can never fire.

That is the §10 pattern at its largest: a tested state machine, a tested
one-shot reporter, and a whole codec module, none of it reachable from the
binary.

**STATUS (c20unified, 623f42518 + 53195b3b3): done, all three steps.** The
binary reports `EncoderReady(codec.encoder_ready())`; snapclone-egui links
snapclone-audio with `opus` (default feature); outgoing voice is one Opus
packet per frame (`Packet::VoiceOpus`, wire kind 4), incoming is decoded per
speaker (`voice_codec`). Not yet tried in a call between two devices.

### Order of work

1. **Tell the truth first.** `EncoderReady(false)` while the payload is raw
   PCM, so the existing warning fires and the state matches reality. One line,
   and it makes the app honest before it makes it fast.
2. Add `snapclone-audio` with `features = ["opus"]` to `snapclone-egui`.
3. Swap the two conversions the comment names. The comment is explicit that
   these are the only place bytes and samples meet, which is why this is a
   contained change rather than a rewrite.

Step 1 is worth doing on its own even if 2 and 3 wait: right now the app claims
a codec it does not have, and the code that would say otherwise is already
written and already tested.

## 15. CORRECTION to §13: snapclone HAS an AV1 video pipeline — in Qt

I wrote in §13 and in c8548d85 that video "cannot be done yet" because
"nothing CAPTURES video". That is true of the egui port and false of the app.
Corrected after being pointed at the Qt tree, and the correction changes the
size of the job substantially — it is a PORT, not a feature.

### What already exists

`apps/snapclone/src/ui/video/` is about 4,000 lines of working pipeline:

* `VideoEncoderBackend.cpp` — SVT-AV1 through **two** routes, gstreamer
  `svtav1enc` and ffmpeg, with element-existence probes before either is used
  (`gstElementExists("svtav1enc")`), which is the same preflight idea
  `d2x-job` needed and got later
* `VideoRecorder.cpp`, `VideoTranscoder.cpp`, `VideoSnapViewerDialog.cpp`,
  `RecordingMetadata.cpp`
* `include/ui/video/Av1Policy.hpp` — CRF/bitrate, GOP, hierarchical levels and
  temporal filtering, chosen per `EncodeTier`

### And the policy is ALREADY IN RUST

`Av1Policy.hpp`'s own header says it: "Mirror of
`snapclone_av1policy::plan_for`". The Rust crate exists at
`apps/snapclone/rust/snapclone-av1policy` — 278 lines, **9 tests, all
passing** — with a C ABI entry point (`plan_for` plus a code-taking wrapper) so
the C++ side can call it rather than reimplement it.

So the DECISION layer of AV1 encoding is already ported, tested, and shared.
What is missing in the egui port is everything around it: a capture path that
produces frames over time rather than one, an encoder invocation, and a
container writer.

There are two identical copies of the crate (`apps/` and `system/`), byte for
byte. Worth folding into one before either is depended on, or they will drift
the way the two `camera-bridge` copies already have.

### What the egui port actually needs

1. **Video capture.** The egui shutter produces one `RawFrame`. Recording needs
   a frame sequence with timestamps, which the frame pump already delivers —
   `RawFrame` carries `captured_ms` and `seq` — so this is a sink that
   accumulates rather than a new capture path.
2. **An encoder invocation.** Follow the Qt backend's shape: probe for
   `svtav1enc`, fall back to ffmpeg, report which one honestly. Both are
   subprocesses, so this is where the "no native encoder on the shutter path"
   argument from `media_encode` stops applying — a video recording is already
   a long operation and a process spawn is noise in it.
3. **`snapclone-av1policy` as a dependency**, not a reimplementation. It is
   tested and it is what the Qt app uses.

That ordering matters: step 1 is pure and testable here, step 2 needs a device
with gstreamer or ffmpeg, step 3 is a manifest line.

## 16. snapclone exists TWICE — `apps/snapclone` and `system/snapclone`

Found while going to fold the two copies of `snapclone-av1policy`. The
duplication is not one crate; it is the whole application, and the numbers say
which way it should be resolved.

### Measured

| | apps/snapclone | system/snapclone |
|---|---|---|
| files | 784 | 654 |
| rust crates identical | 29 | 29 |
| rust crates DIFFERING | 11 | 11 |
| files present only here | 132 | **2** |

The two files only in `system/` are `log.txt` and `third_party/.gitkeep`.
Neither is code.

So `apps/snapclone` is a strict superset in content, and on every drifted crate
but one its side is NEWER:

| crate | apps last touched | system last touched |
|---|---|---|
| snapclone-core, databus, network-diag, piggyback, relay, session, storage, vcard | 2026-09-01 | 2026-08-22 |
| snapclone-gesture | 2026-08-30 | 2026-06-22 |
| snapclone-audio | 2026-08-22 | 2026-06-22 |
| snapclone-group | 2026-08-09 | 2026-08-09 |

### The drift is not purely one-directional, which is the catch

`snapclone-session/src/selector.rs` has REAL WORK on the system side that the
apps side lacks — `NetworkBin::of(ip)`, `scoring::Protocol`, and a long comment
explaining that an `else if let Some(u) = relay_url` "was a type error and had
never compiled". Meanwhile the apps side's own last commit is titled "fix
compile error, dead duplicate bin, unused imports".

Both sides received independent fixes for overlapping problems. Deleting either
one wholesale loses work, and that is why this is an entry rather than a
commit.

### Why it matters now rather than later

`snapclone-egui` gained a dependency on `apps/snapclone/rust/snapclone-av1policy`
in b11806b5. Before that, nothing outside each tree referenced either copy and
the duplication was inert. Now one copy is load-bearing and the other is not,
which is how a fix lands in the wrong one.

`system/snapclone/camera-bridge` is also a second copy of a crate this session
put on the shared kit (ea891613) — I edited the `apps/` one. The `system/` one
still has no `uikit-egui` dependency, so it draws boxes for glyphs and cannot
receive an on-screen keyboard, and `font_install_check.py` lists both paths
separately without knowing they are the same program.

### VERIFIED per crate: there is almost nothing to harvest

§16's first draft warned the drift was bidirectional and that deleting either
tree would lose work. I then diffed all eleven, and that warning was mostly
wrong. Recording the check so nobody repeats it:

| crate | what differs | harvest? |
|---|---|---|
| session | system has `let bin = NetworkBin::of(ip)` — UNUSED, plus its two unused imports | **No.** That is literally the "dead duplicate bin" the apps side's own commit title says it removed. The apps side also skips empty relay URLs, which the system comment explicitly declined to decide. apps is the later call. |
| network-diag, relay, databus | system adds explanatory COMMENTS; apps already declares `chrono`, `base64`, `futures-util` and already has `required-features = ["bin"]` | No |
| piggyback | system adds `serde-big-array` for a `[u8; 64]`; apps stores the signature as HEX and needs no such dep | No — apps solved it differently and arguably better |
| storage | `shown_name` vs `display_name`, same logic | No |
| vcard | `name.to_string()` vs `(*name).to_owned()` | No |
| gesture, group, audio | apps DEPENDS ON THE SHARED KITS (`gesture-kit`, `snapcore`, `audio-kit`) where system has local copies of the same code | No — apps is ahead; this is the fleet-wide kit extraction |
| core | comments only | No |

So the apps tree is not merely newer by date; on every drifted crate it is
newer in substance, and on three it has already been moved onto the shared kits
that the system copy predates.

The one thing worth reading before archiving is `snapclone-session`'s empty
relay URL: apps `continue`s past it, system passes it through. Both are
defensible and the apps side is a deliberate later change, so this is a note
rather than a blocker.

### The resolution, and the one question it needs answered

Mechanically: `apps/snapclone` survives, `system/snapclone` is archived per
§7b, and the eleven drifted crates are diffed one at a time to carry anything
the system side has that the apps side does not — `selector.rs` at minimum.

### The manifest answers it, and the answer inverts the conclusion

`manifest/projects.toml` declares them as different things:

    [apps.snapclone-egui]  path = "apps/snapclone/egui-frontend"
        notes = "...the C++ camera/peer engine (src/, cmake/) is the separate
                 on-device native app; this is the egui reference/demo surface."

    [system.snapclone]     path = "system/snapclone"
        role = "system-service"   daemon = true
        notes = "DHT/identity/relay; canonical Rust<->C++ DataBus FFI;
                 cross-build to FP5/postmarketOS"

So `system/snapclone` is the DEPLOYED DAEMON — the thing that ships to the
phone, carrying the "canonical" DataBus FFI — and `apps/snapclone` is
registered only as the egui reference surface plus a separate native app.

That inverts the file-count conclusion. The newer code is in the tree the
manifest calls a reference surface, and the older code is in the tree the
manifest calls canonical. Eight crates were last touched in `apps/` on
2026-09-01 and in `system/` on 2026-08-22, so **ten days of fixes may be
sitting outside the tree that deploys.**

This is now a question about which tree is meant to receive fixes, and the
manifest and the commit dates disagree. Both readings are defensible and they
lead to opposite merges, so it needs a human answer before anything is
deleted:

* if `system/` is canonical, the nine days of `apps/` fixes need porting INTO
  it, and `snapclone-egui`'s new dependency on
  `apps/snapclone/rust/snapclone-av1policy` points at the wrong copy;
* if `apps/` has become the working tree in practice, the manifest's
  `[system.snapclone]` notes are stale and `system/` should be archived.

What is not defensible is leaving it: one copy is now load-bearing for
`snapclone-egui` and the other is what deploys.

### ANSWERED: the apps tree is what is being used

Two independent confirmations:

1. The operator, directly: "I have been testing what gitmind-egui and apps
   folder use."
2. **gitmind resolves it that way regardless of the manifest.**
   `cockpit::ops::resolve_app_dir` searches `["apps", "games", "system"]` in
   that order and returns the first hit, so a bare name `snapclone` resolves to
   `apps/snapclone` and `system/snapclone` is unreachable by name. The build
   tool has been pointed at `apps/` all along.

That second point is the interesting one: the manifest says `system/snapclone`
is the canonical deployed daemon, and the tool that does the deploying cannot
reach it by name. The `[system.snapclone]` notes are stale, not the tree.

### DO NOT ARCHIVE IT. The tier is the design, and I nearly deleted it.

Corrected after the operator pointed out what `system/` is FOR: a daemon tier —
long-running services on a shared Linux fabric that apps talk to over IPC —
rather than a second place to keep applications. `system/` holds exactly two
entries, `photosync` and `snapclone`, and both are `daemon = true` in the
manifest.

That reframes everything above. The freshness analysis is still correct and
still says `apps/` has the newer code. It does not follow that `system/`
should go, because the two trees are not competing copies of one thing — one
is a GUI application and the other is meant to be the background service it
talks to.

Deleting it because its Rust crates are ten days behind would have removed an
architectural tier for a reason that has nothing to do with the tier.

### The actual bug is the FORK, not the directory

`system/snapclone` is supposed to be two daemons. Its own units say so:

    snapclone-bus.service          ExecStart=%h/.local/bin/snapclone-bus
    snapclone-camera-bridge.service ExecStart=%h/.local/bin/camera-bridge

What it actually contains is 654 files including `src/ui/` — **85 files of Qt
GUI** — a full `CMakeLists.txt`, the whole 40-crate Rust workspace, resources,
packaging and a flatpak manifest. It is a fork of the application that happens
to also build two daemons.

And `apps/snapclone/systemd/` carries copies of the SAME TWO UNIT FILES, so the
GUI tree also claims to install the daemons. Both trees build a binary called
`snapclone-bus` from their own `snapclone-databus` crate.

So the question is not "which tree survives" but "what should live in the
daemon tier". The answer the design implies:

* `system/snapclone` keeps the units, the bus and the camera bridge, and the
  crates those two binaries actually need.
* Everything else — Qt UI, the egui frontend's siblings, resources, packaging,
  the 30-odd crates neither daemon links — comes out.
* The crates BOTH sides need move to `shared/` or are depended on across the
  tree, which is the pattern `apps/` has already started: its `gesture`,
  `group` and `audio` crates now depend on `gesture-kit`, `snapcore` and
  `audio-kit` instead of holding local copies.
* `apps/snapclone/systemd/` loses its copy of the two units, since installing a
  system daemon is not a GUI's job.

That is a real refactor with a clear shape, and it is the opposite of the
one-commit deletion §16 was heading toward two paragraphs ago.

## 17. NEXT CHAPTER: gitmind as a fleet deployer — what exists and what does not

Scoped after being told this is the next chapter. Surveyed rather than assumed,
because most of it turns out to be built already and the missing pieces are
specific.

### Already there, and more than expected

`cockpit/src/deploy.rs` is 1,567 lines and covers the transport end well:

* `scan_ssh` over a /24, `local_subnet24`, device discovery
* push AND pull, `rsync` first with `scp` fallback, and a legacy `-O` fallback
  for modern OpenSSH against old devices
* `remote_capture` for running a command and reading its output
* `parse_authorized_keys` / `dedupe_authorized_keys` — gitmind-managed key
  entries with expiry stamps, so key installation is already modelled
* `rustup_bootstrap_script(target)` and `bootstrap_rustup_remote`

`buildflags.rs` already names the fleet's targets —
`aarch64-unknown-linux-musl` and `-gnu` — and builds a `cargo build --target`
argv, with a comment noting no container is needed when the target std is
installed.

`fleet.rs` has the fleet user, the build repo list and their config files.

So SSH, keys, discovery, transfer and cross-target argv are NOT the gap.

### The gap: nothing installs system integration

`daemons.rs` generates units — `systemd_user_unit(d, exec_path)` and
`openrc_service(d, exec_path)`, with an `InitSystem::detect_from_os_release` so
it handles postmarketOS's OpenRC as well as systemd.

**Neither function has a single caller.** `grep -rn systemd_user_unit` outside
its own module returns nothing. §10's pattern, in exactly the module this
chapter needs: unit generation is written, tested-adjacent, and unreachable.

### And the daemon registry does not know the real daemons

`daemons::all()` lists five fabric services — voiced, sharemd, coordd,
settingsd, logscope-fabricd — with the note "adding one here is all that's
needed for install/start to pick it up".

It does not list `snapclone-bus` or `camera-bridge`, which are real daemons
with real hand-written unit files in `system/snapclone/systemd/`, nor
`photosync-daemon`. So the two `system/` projects — the entire daemon tier per
§16 — are invisible to the daemon machinery.

That is the join between this chapter and §16. `system/` is where daemons live;
`daemons::all()` is what gitmind can install; the two lists have no overlap.

### The chapter, in dependency order

1. **Teach `daemons::all()` about the `system/` tier.** Either extend the
   static list or derive it from `manifest/projects.toml`'s `daemon = true`
   entries — the manifest already marks them, and a second hand-maintained list
   is how the two disagree.
2. **Call the unit generators.** `systemd_user_unit` / `openrc_service` exist
   and nothing invokes them; installation should generate a unit rather than
   copy a checked-in one, so `X-Fleet-Integration` is always present and
   `ExecStart` always matches where the binary actually landed. That also fixes
   the rusty-board class of bug found today (33b48880): a hand-written unit
   whose `ReadWritePaths` named directories nothing created.
3. **Cross-build locally.** `buildflags` produces the argv; what is missing is
   the linker and sysroot story. `bootstrap_rustup_remote` suggests the current
   answer is "build ON the device", which works and is slow — an FP5 building
   unendlich is not a short wait. A local `aarch64-unknown-linux-musl` build
   needs a cross linker, and musl is the easier of the two because it avoids
   glibc version skew against whatever postmarketOS ships.
4. **Install units as part of deploy**, with the daemon stopped first and
   started after, and `daemon-reload` between — the step whose absence made
   today's rusty-board redeploy report a stale unit.

Steps 1 and 2 are pure and testable here. Step 4 needs a device.

Step 3, checked in c18unified: `gcc-aarch64-linux-gnu` and `musl-tools` both
install clean via apt — the linker half is not the blocker it looked like.
The blocker is the Rust STD for the target triple: this sandbox's toolchain
was installed by `pack-rust.sh`'s offline bundle rather than rustup, so there
is no `rustup target add`, and fetching `rust-std-<ver>-aarch64-unknown-
linux-{gnu,musl}.tar.xz` directly from `static.rust-lang.org` — the exact
component `pack-rust.sh` itself downloads — 403s with `x-deny-reason:
host_not_allowed`. That domain is not on this sandbox's fixed network
allowlist (`crates.io`/`static.crates.io` are; `static.rust-lang.org` is
not), and unlike every other "not installable here" claim this session, this
one is a genuine hard boundary rather than an untested premise: the allowlist
is fixed by the harness, not by a package manager.

So step 3 is buildable the moment a rust-std for the target exists on disk —
via `pack-rust.sh` run somewhere with that domain reachable, or a std
fetched through `crates.io`'s proxy if one exists, or vendored into the repo
alongside the existing `rustup.zip`-style bundles. Nothing about the linker,
the toolchain version, or the target triple is in question; only the std's
transport is blocked.

## 18. gitmind LAN share: three independent failures, and where the fix stops

Reported as "curl -O <addr> on the phone didn't work". It was three separate
things, any one of which produces that symptom, and the operator had no way to
tell them apart.

### 1. The advertised address served nothing at `/`

`netshare::serve_loop` answers exactly one path, `/card`, and 404'd everything
else with "unknown path". The UI advertises a bare `host:port`, so the obvious
command hits `/`, and `curl -O` on a URL with no filename reports "Remote
filename has no length" before printing the body. Fixed in 6e44c224: `/` now
returns a plain-text page explaining what the endpoint is and giving the two
commands that work.

### 2. The advertised address could be one no peer can reach

`local_ip()` reads back the interface holding the DEFAULT ROUTE. Wrong twice:
a VPN holding the route advertises a `100.64/10` the phone cannot see, and no
default route at all makes it return `None` — after which the caller fell back
to `127.0.0.1` and displayed it as if it were fine. Fixed in 75357fbc
(ranking) and 2ee0ce56 (enumerating real interfaces, and refusing to advertise
when nothing usable exists).

### 3. Packets can be dropped at either end

The desktop's own firewall is now caught by `netshare::self_check`
(201d2cd3), which connects to the host's OWN LAN address — not loopback, which
bypasses the INPUT chain and would report success on a machine dropping every
LAN packet.

The device's firewall is `device_firewall` + `deploy::ensure_port_open_remote`
(a967eccd, this commit). awall is the one that matters: postmarketOS uses it,
it COMPILES policies into the ruleset, and `awall activate` discards any raw
iptables rule — so the naive fix works and then silently stops.

### Where this stops, and what needs a device

**The awall policy schema is written from documentation, not from a machine
that accepted it.** If a field name is wrong, `awall enable` fails and the port
stays shut. That is the single highest-value thing to check, and it is one
command on the phone:

    # run the generated command, then:
    awall list        # gitmind-share should appear, enabled

Also not done: nothing CALLS `ensure_port_open_remote` yet. The function and
its reporting are tested; the bootstrap flow does not invoke it, deliberately,
because wiring it before the schema is confirmed would put an untested remote
command in the path of every deploy.

### A test-writing note this thread produced twice

Two assertions passed on the wrong thing, both substring matches where the
wrong value contained the right one:

* `contains("/card")` was satisfied by a page printing `/cards`
* `contains("9876")` was satisfied by a message that named the port elsewhere
  while the `ufw` command lost it

Both are now line- or phrase-exact. For ports, paths and identifiers the wrong
value being a superstring of the right one is the common case, not the rare
one — a substring assertion there is close to no assertion at all.


## 19. bscale seam blending: what the frame-resident rewrite costs to restore

Recorded because merging c14unified-026's dandere line (67ed5800) dropped it,
and the reason it cannot simply be re-applied is architectural rather than a
matter of effort. Without this note the next attempt re-derives it.

### Where it used to hook

The replayed seam work (c2a3b204, 4fde955f) composited on the HOST: tiles were
read back one at a time, `Blender::add` accumulated each into a weighted output,
and `tiles_overlapped` stepped by `tile - pre_pad` so the context each tile
already computes was reused rather than discarded.

### Why that hook no longer exists

The new path never reads a tile back. `shaders/pack.wgsl` writes each tile's
output **cropped and addressed straight into a frame-sized device buffer**
(`dst_x`, `dst_y`, `out_stride`), and the frame is read back exactly once at
exactly its own size. That is where the 11x comes from — one submit and one
readback per frame instead of one per tile.

So compositing has moved onto the GPU, into a shader that OVERWRITES. There is
no per-tile host step left to blend in.

### What restoring it actually requires

Not a Rust change. `pack.wgsl` would need to:

1. take a weight per output pixel — a separable ramp over the overlap region,
   derivable from `crop_*` and the overlap width, no texture needed;
2. ACCUMULATE rather than store — `out` is `array<u32>` RGBA8, so weighted
   accumulation needs either an f32 accumulation buffer plus a normalisation
   pass, or atomics; f32 plus a pass is the simpler and is one extra
   frame-sized buffer;
3. and the tile walk moves from `tiles` to `tiles_overlapped`, which is the
   only Rust-side change and is one line.

The normalisation pass is also where the RGBA8 conversion would happen, so it
replaces rather than adds a step.

### What survives, and what to check first

`Blender` and `tiles_overlapped` are still in `d2x-core/net.rs` with their
tests passing — the arithmetic and the weight ramp are intact and are what the
shader would implement. The CPU path still blends.

`selftest` keeps the comment explaining that kernel parity must be measured
with blending OFF (93.3% exact, worst delta 3, which was the blend working);
whatever turns it on must turn it off there.

**Check the artefact is back before doing any of this.** The fix addressed the
fainter STEADY seams; the flickering ones were the residual bleed ring, fixed
separately in f0049067 and untouched by the merge. If the steady seams are not
visible on real footage at the tile sizes now in use — the default tile grew to
688-1024 in this line, from 40-64 — the work may not be worth a shader rewrite.
That is one screenshot, and per `docs/agents/HEADLESS_GUI_TESTING.md` this
sandbox can take it.


## 20. The sandbox CAN see a GUI, and a dozen of my claims today were unnecessary

`docs/agents/HEADLESS_GUI_TESTING.md` arrived in c14unified-026 correcting
CLAUDE.md's "GPU, Qt and GTK paths cannot be built in this sandbox". I tested
the correction rather than believing it, and it is right.

    apt-get install -y --no-install-recommends xvfb openbox xdotool imagemagick
    export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
    Xvfb :99 -screen 0 1280x800x24 &   # then openbox, then the app
    import -window root /tmp/shot.png

**switcheroo-egui built, launched, rendered and screenshotted.** Roughly seven
minutes for a cold debug build, seconds for everything else. `openbox` really
is required — without a window manager `xdotool windowactivate` fails and
synthetic keys go nowhere.

### What the screenshot settled instantly

Two things I committed blind today, both confirmed:

* **The symbol fonts reach switcheroo.** `☆` and `↻` render as glyphs, not
  tofu boxes. That was the whole claim of 51e910c8 and I had no way to check
  it — cropping the pin button to 40x40 shows a star.
* **The palette applies.** Dark theme, orange warning text for the missing
  Bluetooth socket, accent on the active tab. The `Colors` indirection from
  that commit is doing what it was written to do.

It also showed the touch targets are the right size beside the toggle, which
was the other half of that commit.

### The claims that are still unverified but no longer unverifiABLE

Every "NOT verified by eye — no display here" I wrote today was avoidable:
snapclone's fullscreen camera and 90° rotation and dark capture flash,
aperture's palette scroll and its two regressions, logscope's boxed glyphs,
photosync's grid. The camera ones need a camera and would show chrome layout
at least; the aperture and logscope ones need nothing but a build.

The honest summary is that I inherited a claim, repeated it a dozen times, and
never spent the one `cargo build` that would have tested it — while spending
the whole session telling other people's code to check its assumptions.

### Cost, so the next session budgets it

A cold debug build of a small egui app is ~7 minutes here and must be
backgrounded across tool calls. The X server, window manager and capture are
seconds. Disk is the real constraint: a debug target dir is 1-3 GB and this
sandbox hit genuine `No space left on device` twice today.


## 21. Fleet test run: 10 of 59 failed, and one failure is the harness

From `fleet-logs.zip`, a full run of bundle 006 on the device. Triaged rather
than fixed, because most need the hardware.

### The one that is not a code failure

**`uikit_egui` cannot be found by rustdoc**, in TWO crates —
`rusty-board-egui` and `logscope-egui`. Both are `E0463 can't find crate`, both
only in the DOCTEST target, and both list a correct `--extern uikit_egui=…`
in the rustdoc command line. The crate is a plain path dependency in each; the
lib target compiled fine seconds earlier.

The build directory is **`/dev/shm/fleet-build`** — tmpfs, so RAM. The likely
cause is that it filled and some rlibs were lost or truncated while others
survived, which fits the shape: two specific crates missing, everything else
resolving.

`uikit-egui` is a plausible casualty. It embeds 566 KB of subsetted Noto fonts
(`NotoSansSymbols2-uisubset.ttf` 364 KB, `NotoSansMath-uisubset.ttf` 202 KB)
that I added in 7098bc4a, so its rlib is among the larger ones in the fleet.

`logscope-crates-gpu-gemm` also died in `/dev/shm/fleet-build` with a SIGSEGV,
which is consistent with the same pressure.

**Check before chasing this as a Rust problem:** `df -h /dev/shm` during a
fleet build. If it is filling, point `CARGO_TARGET_DIR` at disk — a full fleet
debug build is several GB, and this sandbox has hit disk exhaustion twice today
with far less in flight.

### Real failures, by app

| app | failure |
|---|---|
| snapclone-egui | `camera_enumerate` asserted `id.starts_with("video")`; the device returns a PipeWire `v4l2_input.platform-…` id. **Fixed in b1775838.** |
| beatspace | `instruction requires: fullfp16` — an aarch64 codegen flag, not a test |
| bo-suite | `speaker_calibration::label_updates_on_subsequent_calibration_if_empty` |
| bot-arena | `delete_local_removes_cached_model`, and `shelter_does_not_sandbox_bwrap_out_of_existence` — the second needs bwrap |
| logscope | `a_range_read_of_a_huge_file_does_not_allocate_it`, `compile_grades_real_code` |
| studio | three `studio-video` probes — silent-video audio, thumbnail strip, PNG bytes; all need ffmpeg |
| gpu-gemm | SIGSEGV, see above |

### I reproduced the tmpfs failure by accident

Re-running this session's crates WITHOUT `--lib`, the sandbox hit
`No space left on device`:

    error: failed to build archive at `.../libeframe-….rlib`: No space left on device
    error: could not compile `eframe` (lib)

One rlib fails to write while everything else has already succeeded. Same
signature as the fleet run's `E0463`, seen from the other end: here the write
failed loudly, there it presumably failed earlier and left rustdoc looking for
a file that was never finished. Same cause, different symptom depending on
which step notices.

Four disk exhaustions in this sandbox today, all with a smaller working set
than a full fleet build. That is the argument for a disk-backed
`CARGO_TARGET_DIR`.

### What this run proves about `--lib`

Both `E0463`s are in the **doctest** target. `cargo test --lib` does not build
doctests and would report every one of these crates green — which is the
warning c14unified-026 arrived with, demonstrated on the same day by an
independent run. I used `-q --lib` throughout this session.

Acted on rather than noted. I re-ran the two crates I changed most today with
a plain `cargo test`:

* `uikit-egui` — 377 lib, 1 doctest, 2 ignored.
* `snapclone-egui` — 1644 lib, plus a **3-test integration target** `--lib` had
  been skipping all day.

Both clean, so nothing was hiding. But those three had not run since I started
changing that crate this morning, and I reported the count as though they had.


## 22. The aperture palette, finally looked at — and it took four tries

Closing the loop on a report that took four commits, three of which were aimed
at the wrong widget.

### What was actually wrong

There are FIVE result lists in that palette, one per `Mode`. The report was
about `GoToRoute` (`Ctrl+G`). I fixed `Commands` and `OpenFile` twice — the two
I had read — while screenshotting a third and believing it was the first. The
hint text is what gave it away: `"describe what you're looking for…"` is
GoToRoute's, `Commands` says `"command, theme, or :line…"`.

All five now share one shape: `max_height(PALETTE_LIST_H)`,
`auto_shrink([false, true])`, `VisibleWhenNeeded`, one `id_salt` each.

### Verified by eye, at last

Running the binary against a real file, `Ctrl+O`:

* **Many matches** — eleven rows, capped at `PALETTE_LIST_H`, scrollbar present
  because there is more below. Correct.
* **Narrowed to nothing** — the box collapses to the input plus one line of
  hint text. That is the "responds to the content" behaviour that was missing,
  and it is the case the old fixed-height version got wrong.
* **No gutter** when there is nothing to scroll; a bar when there is.

### The method note, because it is the transferable part

`cargo check` was clean for all four attempts. Reading was clean for three. The
thing that found it was launching the app and reading a string on the screen.

`docs/agents/HEADLESS_GUI_TESTING.md` arrived the same morning and I used it
three commits late. Costs: ~15 minutes to link aperture's 434 MB debug binary,
seconds for everything after.

Two harness traps worth knowing:

* **`xdotool type` drops characters.** `type "save"` landed `sa`. Sending keys
  one at a time with a delay is reliable; `type` is not. Several of my "no
  matches" results were the harness, not the matcher.
* **Xvfb dies quietly.** Twice the app failed with `XOpenDisplayFailed` and the
  server was simply gone. The error names the display, not the dead server, so
  check `ps aux | grep Xvfb` before believing the app is at fault.


## 23. logscope-egui mesh artefacts: backface culling is never enabled

Reported as "we have some mesh issues in logscope-egui", with a screenshot
showing spiky black wedges and odd shading through the scene.

Investigated rather than fixed, because the obvious one-line change could make
it worse and I could not verify it in the time I had.

### What I ruled out

**Not NaN normals.** `scene_kit::mesh3d::norm` guards a zero-length vector and
returns `+Y` rather than dividing by zero — and because `NaN > 1e-6` is false,
a NaN input takes the same safe branch. The vertex-normal accumulation in
`rock` and friends is area-weighted by construction (the cross product's
magnitude IS the area), which is the correct scheme, not a bug.

That matters because blackstar's non-finite audit (c14unified-026) found five
defective modules of nine, so it was the first thing to suspect here. scene-kit
is clean on this point.

### What I found

`shared/scene-kit/src/glow_scene.rs` enables `DEPTH_TEST` in two places and
**never enables `CULL_FACE`**. Grep returns nothing for it.

With depth testing on and culling off you do not get wrong ORDERING — the depth
buffer handles that — but every back face is rasterised and shaded. Where a
mesh is open, or its winding is inconsistent, you see its interior lit as
though it were exterior. That is consistent with the artefacts in the
screenshot, and it also doubles the fragment work in a scene this dense.

### Why the one-line fix is not obviously safe

`gl.enable(glow::CULL_FACE)` is correct only if every mesh winds consistently.
`mesh3d` has 32 places pushing triangles; the parameterised primitives (sphere,
rock, tube) derive their topology and will be consistent, but the hand-built
quads and panels each spell out `[0,1,2],[0,2,3]` and any one of them written
against a flipped basis would VANISH rather than shade oddly.

So enabling culling turns a shading artefact into a missing-geometry artefact
if the assumption is wrong, and there is no compiler check for it.

### The experiment to run

This is now cheap, per §22: build `logscope-egui`, run it under Xvfb, screenshot
the scene, enable `CULL_FACE`, screenshot again, and compare. Ten minutes of
build and two screenshots settles both questions at once — whether culling
fixes the artefacts, and whether anything disappears.

If geometry does vanish, the follow-up is `glow::FRONT_AND_BACK` culling
disabled for the specific meshes at fault rather than reverting wholesale, and
the vanished ones are then a list of primitives to fix the winding on.

### UPDATE — the experiment ran; a real bug got fixed; the original one is still unconfirmed

Ran the §23 experiment across three follow-on commits (4db4e306, 1455743e,
da78594d): built `scene-view`, got real screenshots out of Xvfb+llvmpipe
(non-trivial — see `scene_preview.rs`'s doc comment for why plain
`import -window root` doesn't work here), A/B'd `CULL_FACE` on vs off with a
camera pinned via `LOGSCOPE_CAM` so the auto-orbit director couldn't drift
the two captures apart.

**Culling: nothing vanished.** Same architecture, HUD, decorative elements on
both sides. None of `mesh3d`'s ~32 hand-built quad sites has backwards
winding. Culling is unconditional now, per this section's own stated rule.

**But along the way, a different, real, verified bug got found and fixed**:
heavy rainbow banding across the Atlantis sky/water gradient, isolated by
bisection to the nacre/thin-film-iridescence term (`LOGSCOPE_NACRE=0` made it
vanish completely) and root-caused to that term's phase frequency being tuned
for a small facet (a pearl, a window pane) but applied to `is_water`'s full
scope (floor, dome, outer walls — tens of world units), so the phase
completed several full rainbow cycles across one continuous surface. Fixed
by cutting both phase frequencies ~10x (da78594d).

**Important gap, flagged rather than papered over**: this section's own
report text says "spiky black wedges and odd shading" — not colour banding,
not rainbow, not a wash. Neither this session's screenshots nor the
immediately-prior session's baseline (its own commit's description: "a heavy
cyan-white wash... over the whole frame") show anything matching "spiky
black wedges." That phrase is this session's inheritance of the PRIOR
session's own paraphrase of what it saw, not a re-read of this original
report — and on going back to check, they don't match. Neither of us has an
actual copy of the original screenshot to compare against.

So: two real, verified fixes landed (culling made safe+unconditional; a
genuine rainbow-banding bug found and fixed). Whether either one is THE bug
originally reported is unconfirmed. If "spiky black wedges" recurs, it's
still open, and worth looking at with fresh eyes rather than assuming either
of these fixes touched it — degenerate/near-zero-area triangles (genuinely
"spiky") or a specific theme/scene state neither session happened to render
are both still on the table.


## 24. bscale blocky artefacts: the merge hard-copies every residual block

## 24b. dandere2x block reuse: measured record, and the seam risk region redraw adds

The reuse work (region redraw, anchor propagation, sub-pel refinement, the
plate, subtile cache harvesting) is written up in
`apps/dandere2x/docs/REUSE_FINDINGS.md` — clip-by-clip numbers, and which of
the obvious ideas measured as losses. Read it before adding a block source.

Two things there that touch §24 directly:

- Region redraw blends one rectangle back with the same `blend_block` ramp,
  now via `blend_rect`. Whatever §24 concludes about the per-block crossover
  applies at the rectangle's edge in the same way, on one seam per frame
  instead of hundreds.
- Every number in that document comes from `analyze`, which counts pixels and
  cannot see a seam. The anchor gate (`--anchor-rmse`, flat RMSE) is looser
  than the JPEG gate for smooth content, and a cached hit is a block drawn in
  another frame's context. Neither has been checked with `compare` on a GPU.

## 25. Fleet run on 7c82c39c: 3 build / 9 test failures, and what each one is

The first on-device run (Asahi aarch64, Fedora rustc 1.97.1) after the five
sibling sessions — voiceterm, ls008, aperture/030, studio 0189–0200, v028,
blackstar — were merged onto the queen/probe series in `53e807f2..7c82c39c`.
61 crates build, 52 test crates green, 6877 tests passed. Every failure below
was attributed by checking whether the merge range touched the failing code,
and by rebuilding the base where a number needed a baseline. Attribution, not
fixes: all but one need the hardware or the host.

### Not the merge: was failing before it (§21 already lists most)

| where | failure | evidence |
|---|---|---|
| beatspace, +features | `gemm-f16`: `instruction requires: fullfp16` | §21; aarch64 codegen flag; `apps/beatspace` untouched in range |
| bo-suite | `speaker_calibration::non_finite_correlation_rejected` | §21 had a different test in the same module; file untouched in range |
| bot-arena | `experience_log_dedups_identical_fixes`, `registry_scans_gguf_and_local` ("gguf scanned" — wants a local models dir) | files untouched in range |
| logscope-fan | `verdict::compile_grades_real_code` — Pass expected, got Fail | §21, "triaged, not root-caused". v028 `114ea3e6e` fixed only the no-rustc case (127 → None); this host HAS rustc, so the cause is different. **Passes on x86_64 rustc 1.96 here.** Cheapest next step, on the device: `echo 'pub fn a(){}' > /tmp/x.rs; rustc --crate-type lib --edition 2021 --emit=metadata -o /dev/null /tmp/x.rs; echo $?` — if that is non-zero, `DEFAULT_CHECK`'s `-o /dev/null` with `--emit=metadata` is what 1.97 or Fedora's build rejects, and the fix is a real temp path. |
| logscope-egui | `long_horizon_tests::a_range_read_of_a_huge_file_does_not_allocate_it` — 51 MiB RSS growth on a 31-line read of 256 MiB | §21. `read_range` unchanged in range. The device's 16 K pages make RSS deltas coarse; the assertion may need a page-size-aware bound rather than a fixed one. |
| logscope-egui | `sandbox::a_FROZEN_posture_forces_offline_however_the_caller_asked` — only in the no-features run, green in `+features` | `sandbox.rs` untouched. Both runs share `temp_dir()/logscope-posture-test` and the fleet runs them concurrently ("waiting on 2 test runs"): a test-isolation race, not the gate. Give the dir a per-process suffix. |
| logscope | `dupscan::duplicate_names_do_not_grow` — 386, ceiling 379 | **Fixed.** Built the ratchet with ceiling 0 at the fork `4af5f6b7` (379), the pre-merge base `e69a006a` (386) and the merged tip (386, identical set). All seven came from the 009 queen/probe series, whose session never ran `-p logscope`; the merge added none. Audited in `dupscan.rs` next to the new ceiling — none is an implementation twice. |
| av1tuner | `TextEdit::id_salt` ×6 | egui API drift; untouched in range |
| rusty-board | `gtk4-layer-shell-sys` build script | host lacks the dev package; untouched in range (1 TODO.md line) |
| voiceterm `web2qml` | `LocalKey<Cell<usize>>::load` | not a method that exists on `LocalKey`; untouched in range, so it did not compile before either |
| rusty-board-egui, aperture-scene | doctest `E0463 can't find crate` | §21's shape exactly. §21 blamed tmpfs pressure; this run had 94 GiB free in `/dev/shm`, so that theory is weakened. Doctests of path-dep crates under a shared `CARGO_TARGET_DIR` is the pattern to look at next. |

### New tests from the merged work, environment-sensitive

* **aperture** `lsp_incremental_sync_e2e`, `lsp_typing_sync_regression` — both
  time out at 15 s with `popup: None`. They skip when `rust-analyzer` is not
  on PATH, so on this host it IS present and did not answer in time. The
  sender verified them against rust-analyzer; whether this is r-a startup
  time on aarch64, a version difference, or a real sync bug is not decidable
  from the log. Run one with `RUST_LOG=debug` and a longer deadline before
  suspecting the sync.
* **gpu-gemm** `window_batching_tests::flush_every_bounds_submits_and_does_not_change_results`
  — expected a flush every ~4 dispatches over 30, got 14 submits. Test
  unchanged in range; this is the FIRST time gpu-gemm's suite has run to
  completion on the device (§21: SIGSEGV). "Apple M2 Ultra … subgroup decode
  DISABLED" is in the log, so the Asahi driver is in play. A device
  observation, needs the device.

### Fixed after §25 was written (same session, commit after this one)

| what | root cause | fix | verified |
|---|---|---|---|
| `compile_grades_real_code` | **`rustc --emit=metadata -o /dev/null` makes its atomic-write scratch dir NEXT TO the output — `/dev/rmetaXXXX` — which only root can create.** The sandbox is root; the device is not. Reproduced as `nobody`. | `verdict::compile()` runs the checker in a per-call scratch dir; `DEFAULT_CHECK` drops `-o /dev/null` | 440 fan tests as root; verdict tests as an unprivileged user |
| the three VmHWM tests | process-wide high-water mark + parallel test threads | each re-execs the test binary `--exact <self> --test-threads=1` and measures in the child | pass; broken on purpose with a 64 MiB hog → reports 65 MiB |
| `sandbox::a_FROZEN_posture…` | fixed temp-dir names shared by the two concurrent fleet runs | per-process suffix (4 dirs across sandbox/review_gate/workspace) | pass, lean |
| `logscope-egui --no-default-features` | `runcard`/`nodelabel` gui-gated but pure; `panels-view`/`shell-view` auto-discovered bins with no `required-features` | ungated both; `[[bin]]` entries with `required-features = ["gui"]` | lean, lean+wgpu-engine, default all check |
| av1tuner `id_salt` | lone egui 0.28 pin in a 0.34 tree | 0.34; empty `fn ui` per the 0.34 `App` trait (voiceterm-egui's shape); `from_id_salt`; `SliderClamping::Always` | check clean, 97 tests |
| web2qml `LocalKey::load` | `TABLE_COUNTER` is a `thread_local! Cell`, not an atomic; never compiled | `.get()`; also a doubled `#[test]` in link_graph.rs | **not built here (Qt)**; API proven standalone |
| bo-suite `speaker_calibration` | global registry + `XDG_DATA_HOME` mutated by 10 parallel tests | `static Mutex<()>` in `with_isolated_xdg` | **not built here (Qt)**; exclusion proven standalone, 8 threads |
| bot-arena ×2 | `CODEBOT_ARENA_HOME` mutated by 7 tests; `engine::env_test_lock` existed and four tests did not take it | they take it | **not built here (GTK)** |
| rusty-board | host lacks the layer-shell dev package; the crate's `wayland` feature already gates it | none — `dnf install gtk4-layer-shell-devel` (Debian: `libgtk4-layer-shell-dev`) | — |

Still needing the device: gpu-gemm `window_batching` (Asahi driver), aperture LSP e2e ×2 (rust-analyzer present but silent for 15 s), the doctest `E0463`s.

### Found while attributing, not a test failure

* **`logscope-egui --no-default-features` does not compile**, and did not at
  `e69a006a` either: ungated `workspace_exec.rs` calls `crate::runcard`, which
  is `#[cfg(feature = "gui")]` (from `d218468d5`, Aug 31). `logscope-cli`
  cannot build lean. The fleet builds default features so never sees it.
* **Two readers for one job.** ls008's `workspace_exec::read_near` reads the
  whole file with `read_to_string` to return an excerpt — it needs the whole
  file for its brace-depth `#[cfg(test)]` scan — while `probe_exec::slice`
  in the same crate streams and stops, and its module doc says why. This is
  the same seam as the two `queen` modules (`logscope_fan::queen` designs
  workers; `logscope_egui::queen` plans probes): the 008 and 009 sessions
  built the model-facing and the queen-facing half of "read near X without
  the whole file" without seeing each other. Unify on the streaming reader
  when the queens are reconciled.
* `gpu_kernels::Model` now carries `arch`, `gaps` AND `moe_variant` +
  `model_gaps()` — two sessions' shapes, superset, see `e0ede4aa`.
  `archfeat::llama_cache_path_has_no_gaps_to_capture` pins the cache-path
  default and was broken on purpose once to prove it fires.

---

# C14_TODO — open threads from the c14 session (blackstar/voice line)

TWO SESSIONS wrote a file called `C14_TODO.md` independently, and both are
kept. Everything above is c15unified (`2accbb2ee..`): the VT corpus, the
dandere merge, the fleet-deploy chapter, the gitmind share failure. Everything
below is the c14 session (`3f1e9e5a..29c36f26`): the licensed-vocabulary
cleanup, smooth terrain, and the voice/linebank pipeline.

They do not overlap in subject and neither supersedes the other, so joining
them beats choosing. The section numbers restart below — an entry is `§N` of
whichever half it is in, and the two halves are told apart by this divider.

Same convention as `C9`–`C13`: every item says how far it actually got. None of
those files are superseded.

Session: `3f1e9e5a..45f7bf26`, 38 commits. blackstar-rs at **1546 green** under
`--no-default-features`. New crates: `shared/opus-kit`,
`games/shared/isosurface`, `games/shared/linebank`.

Three themes ran end to end: **the licensed-vocabulary cleanup finished**,
**smooth terrain became buildable**, and **a voice pipeline exists that has
never touched a microphone but has run on real files**.

---

## 0. THINGS THAT COST THIS SESSION TIME, AND WILL COST YOURS

Read this section before running anything. Every line is something that
presented as a different problem than it was.

**The default feature set does not build in a container.** `cpal` needs
`libasound2-dev` and there is no `sudo`. Use `--no-default-features`, which the
manifest already documents as the silent-binary path. Symptom without it: a
link error that looks like a code error.

**`cargo test` without `--bins` fails before running anything.**
`examples/silhouette_audit` references `crate::anim` and `crate::hardpoints`,
which do not resolve. Three errors, nothing to do with your change, and they
mask the entire suite. Use `cargo test --no-default-features --bins`.

**`--arch emit-c` needs the binary rebuilt FIRST.** Running it against a stale
binary writes a header that looks updated and is not — the file's mtime moves
and its contents do not. Chain it: `cargo run ... -- --arch emit-c && cargo
test ...`. The header check is the only thing that catches this.

**Disk fills at ~5 GB per build tree and there are several.** The container hit
100% twice. The second time presented as `rustc` exiting 101 with "could not
compile" and no error above it, which reads exactly like a code failure. Check
`df` before believing a mysterious compile error. `games/unendlich/target`
reached 5.4 GB, `blackstar-rs/target` 3.2 GB.

**Anything under `apps/studio/crates/*/render/` or blackstar's `render/` is not
compiled headlessly.** `render` is behind `--features render-core`.
`cargo check --lib` passes and tells you nothing; the tests simply are not in
the binary, reported as "N filtered out, 0 passed", which reads like a filter
typo. A module written there is unverified BY CONSTRUCTION. `smooth_vertex.rs`
lost a revision to this before being moved to `src/world/`.

**Backticks in a `git commit -m` message get command-substituted** and silently
delete words from the body. Use `-F` with a file for anything long.

**Test builds are 5–9 minutes.** Run them detached (`setsid nohup ... &`) and
poll, or the harness times out mid-link and you learn nothing.

**The toolchain**: `rustup.sh` wants `static.rust-lang.org`, which is not in the
allowlist, but the zip carries the full offline bundle —
`PREFIX=/home/claude/.rust sh INSTALL.sh` works. `export
PATH=/home/claude/.rust/bin:$PATH`.

---

## 1. THE VOCABULARY CLEANUP — CLOSED, 30 FLOORS REMAIN

~680 licensed marks down to 30, none of which are in-fiction vocabulary. The
live enforcement is **`blackstar-rs/src/vocab.rs`**, a census that asserts an
exact count per term so a removal FAILS and prompts you to lower the number,
and an addition fails too.

`docs/RENAME_MAP.md` is now a record rather than a plan. `docs/films/
CANON_DECISIONS.md` has the full cast under "The cast, named".

### The names, and which are load-bearing

| was | is |
|---|---|
| Autobots / Decepticons | **the Enduring** / **the Sundered** |
| Optimus Prime | **Keelwright** |
| Rodimus / Hot Rod | **Halyard** / **Jib** (Jib becomes Halyard — the name change IS the inheritance) |
| Megatron | **Cleave** |
| Jhiaxus | **Assay** |
| Unicron | **the Unending** |
| the Matrix | **the Charge** |
| Prime (title) | dropped |
| Cybertronian | **Machine** (`Body::Machine`, `machine.rs`) |
| Cybertron | **the Foundry** in the archive; **home** to the Enduring |
| energon | **valence** |

Three pairs stop working if one is taken and the other left:
Enduring/Unending, Keelwright/Halyard, Jib/Halyard.

### Five ways a vocabulary sweep bites, all of which happened

1. **It leaves `src/`.** Boss ids in `assets/raids/bosses.json`, catalogue ids
   in `tools/meta/library.json`, thirteen docs including `STORY_BIBLE.md`
   (whose claim block is asserted against `archive::CLAIMS`).
   `raidcon`'s roster-matches-json test was twice the only thing standing
   between a rename and silent data drift.
2. **It corrupts code that reasons ABOUT the names.** `saga.rs` has two guards
   whose needles are the licensed words. A sweep rewrote them **five separate
   times**, turning a licensed-mark detector into a detector of the replacement
   names — and the symptom was always a count going UP, reading like a
   reintroduction. Now guarded by `vocab::no_sweep_has_eaten_a_guard`, and the
   files are listed in `vocab::DO_NOT_SWEEP` (`saga.rs`, `grandeur.rs`,
   `vocab.rs`).
3. **It falsifies citations.** `grandeur.rs` is a table of measurements from
   real reference material where the LABEL identifies the source. A sweep
   renamed "Optimus Prime" to this project's character, turning a published
   28 ft production figure into a false attribution. Only the ungrammatical
   half of the same sweep ("a the Foundry spire") made anyone look.
4. **Writing about a mark creates an occurrence of it.** Guard needles, policy
   notes, this file. A census cannot tell a use from a mention, so every
   explanation raises a floor. Expected, not alarming.
5. **`\b` does not work.** Underscores are word characters, so `\benergon\b`
   misses `energon_value` and `at_energon`. The first pass left 30 occurrences
   and looked like it had worked.

### The lesson that generalises

**A guard only guards what it enumerates.** The needles were eaten five times
before a test was added. Then the same failure appeared one level up: the
census is case-sensitive by design and nobody enumerated SCREAMING_CASE, which
is exactly where display strings and public statics live — `patrol.rs` had
`name: "CYBERTRON"` on a destination **the player reads**, `survival.rs`
returned `"CYBERTRONIAN"`, `saga.rs` exported `RODIMUS_PRIME_ARC`. Nine in
`src/`, thirteen in the screenplays, all reported as zero.

### The 30 that remain and cannot go lower

Citations in `grandeur.rs` / `compact.rs` / `xform.rs` (~15), guard needles
(6), asset filenames naming a real game (3), policy notes (~6). Each is
documented in `CENSUS` with its reason.

---

## 2. THE WORLDHOST SEAM — three modules across, physics is a decision

`games/shared/worldhost` existed and was used at **three call sites** while
thirteen modules still took `&World`. It was declared more than done.

Now: `footing.rs` and `camera.rs` are through the seam, `src/seam.rs` holds the
list plus a test that reads each module's source via `include_str!` and fails
if `&World` gets back in. **Verified by injecting a regression and watching it
fire** — a guard that has never failed is unproven.

Three methods were added, each because porting needed a fact the trait could
not express: `gravity_dir`, `ground_plane` (because `ground_h` returns `f32`
and so can only say HOW HIGH, never WHETHER), and `platform_vel`. All default
to the flat-world answer.

`worldhost::conformance` is the contract as executable functions, public so
other crates' tests can call it. It runs against blackstar's tile `World`
(MEADOW and KUIPER), the unendlich adapter, and the built-in hosts.

**`worldhost-unendlich` was written against the engine source and had never
compiled.** It does now (`--features engine`). Rewritten into `VoxelSource` (3
questions) plus a pure `Host`, so the geometry tests in milliseconds and the
`ChunkManager` impl is nine lines. Two bugs fixed in the rewrite:

* `ChunkManager::block_at` returns `Air` for a NON-RESIDENT chunk, so a
  clearance built on it called the streaming frontier free space and a body
  walked off the world. `Cell` is three-state now and `Unloaded` defaults to
  `Solid`.
* `ground_h` must return `y + 1`, the top FACE. Returning the index buries the
  body half a block.

**OPEN — physics.** `worldhost`'s header says physics is deliberately outside,
and the numbers agree: 15 in-trait accesses against 75 outside, mostly
`bodies`(18), `normal`(9), `shapes`(8), `launchers`(6). It does not ask "what
is the world like here", it asks "give me your solids". Behind a trait that
costs a virtual call per contact in the inner loop or a shape vocabulary both
engines share. **Do not start it by adding one more method to the trait.**

---

## 3. SMOOTH TERRAIN — headless half complete, render half untouchable here

`games/unendlich/docs/TERRAIN_RESOLUTION.md` is the decision record. Two of its
claims were WRONG and are corrected there:

* Fields are **not** per-cell. `FIELD_RESOLUTION = 8`, so a field grid is 8³ =
  512 samples per 32³ chunk — 2 KB, not 128 KB. The "fields dominate storage"
  argument against a smaller block was not real.
* **The density field already exists.** `WorldModel::density(x,y,z,t) -> f64`
  is the source of truth for solidity and `cpu_generator` builds the block grid
  by THRESHOLDING it at 0.0. The block array is a one-bit quantisation of the
  terrain; the precision a smooth mesher needs was computed and discarded.
  Generated terrain needs **no new storage at all**.
  `density_from_surface`'s own comment says it is tuned to be monotonic through
  the surface — which is exactly what linear interpolation between two samples
  needs. The generator was written to be isosurfaced; nobody had done it.

Built: `games/shared/isosurface` (surface nets, gradient normals),
`world/smooth.rs` (the bridge + materials), `world/smooth_plan.rs` (LOD
planning), `world/smooth_vertex.rs` (12-byte GPU format),
`world/smooth_chunk.rs` (one call, plan → uploadable).

**LOD stitching is solved**, and `TERRAIN_RESOLUTION.md` was wrong that it
would be the bulk of the work. Boundary cells facing a coarser neighbour are
meshed on the NEIGHBOUR'S cell grid (`Seams`) — 1.46 cells of disagreement down
to 0.14 — and the structural remainder closes completely when the fine chunk
meshes one coarse footprint PAST the plane, at which point it owns the
neighbour's cell and emits the bridging quads. Every coarse vertex matches to
within 1e-3.

**A failed approach is recorded in the header so it is not retried**:
quantising the boundary vertex POSITION made the seam WORSE (3.86 → 5.27),
because surface nets encodes the surface location *in* the vertex position, so
rounding destroys the answer.

**`PackedVertex` cannot carry this path** — no normal, and `pack_pos` clamps to
`[0,32]` so every LOD-overlap vertex would silently collapse onto the chunk
boundary. The seam work undone at upload, by a clamp, with no error anywhere.

**Blocked here, not blocked in general**: the Vulkan pipeline binding, the
octahedral-normal shader, and the smooth/greedy choice in `meshing_pool` are
all inside the feature-gated tree.

### 0.5 m blocks

Still open, and now cheaper than it looked. The real blocker is not memory: the
engine has **no block-to-metre constant** and **eight independent gravity
constants in three unit systems** — the player falls at 28 blocks/s² while
loose entities fall at 9.8 m/s², and the two documented block lengths differ by
3x. `crates/unendlich-engine/src/scale.rs` names the scale once and pins the
current state in characterisation tests. It changes nothing: retuning gravity
is a design decision that wants a playtest.

---

## 4. THE VOICE PIPELINE — complete except encode

Everything that does not need libopus is built, tested and composed. It has run
on the real screenplays.

```
cargo run -- --vo                    # the bank, its faults, its size
cargo run -- --vo --write DIR        # a script per actor, ready for a booth
```

Against the real tree: 56 written lines, 16 barks, 5 speakers, 130 takes,
~322 s in the booth, **1.00 MB of Opus against 31 MB as WAV**.

Crates: `games/shared/linebank` (ids, screenplay parsing, script, slicing,
alignment, audit, budget, the whole import in `session::import`),
`shared/opus-kit` (container arithmetic), plus `wav` and `envelope` added to
`shared/audio-kit`.

### Design decisions worth not relitigating

* **Variety is a selection problem, not a content problem.** `Bag` draws
  without replacement and refuses to put the take you just heard first after a
  reshuffle. Five takes through a bag beat a dozen through `rand()`.
* **Units are phrases, not words.** Splicing word-level takes fails on PROSODY,
  not on the joins. `Carrier` models carrier-and-slot and exists mainly to make
  the cost visible: two slots of 4 and 6 is 24 reads, not 10.
* **Ids derive from CONTENT** — speaker plus opening four words. Position-
  derived ids renumber on insertion and ids are permanent. Content-derived ids
  are stable under reordering and change only when the opening words change,
  which is exactly when a re-record is due.
* **Alignment is by order and mismatches are loud.** No slate heuristic: half
  the bank is blips and a blip is SHORTER than the slate announcing it.
* **Encode last.** Record lossless, slice, verify, then encode. Masters are
  never vendored.

### Bugs found in existing code

* `apps/voiceterm/src/acoustic_index.rs::load_wav_16k_mono` advances by
  `p += size` with **no RIFF pad byte**. One odd-sized chunk — exactly what a
  metadata-writing recorder produces — and every later chunk id reads a byte
  off, `data` is never found, and the file SILENTLY does not load.
* `pre_skip` is handled in `beatspace` and `studio-media` independently;
  `studio-media` also handles the **end-granule trim**, which is the one that
  gets missed. On a loop it is a tick every bar; on a bank of 300 ms blips it
  is a soft edge on every one.

### Open

* ~~**Opus ENCODE.** Nothing in the tree encodes Opus; bo-suite's clips came
  from an external tool. Needs libopus behind a feature.~~ **DONE, `5488caba`.**
  `opus-kit::encode` (new `encode` feature, `audiopus` → system libopus,
  isolated to one file — `lib.rs` stays exactly as libopus-free as its own
  docstring commits to). Writes a complete, spec-correct Ogg Opus container:
  real page framing, the correct (unreflected) Ogg CRC-32 — not the
  zip/png/zlib variant — `pre_skip` read from the encoder's actual
  `lookahead()` rather than assumed, and the granule trim matching this
  crate's own read-side convention.

  **A real bug, caught by a test failing rather than by reasoning about the
  RFC**: the first version didn't encode enough trailing frames to flush the
  encoder's own algorithmic delay, silently dropping the last ~6.5ms of real
  audio. Fixed by padding the encoder's input with `pre_skip` extra samples
  so the delayed real content actually propagates out before the container
  is trimmed back to the true length.

  **Verified independently, not just self-consistently**: a real `.opus`
  file written to disk was accepted and correctly decoded by `opusdec` (the
  reference libopus decoder — including reading back the vendor string this
  code wrote into `OpusTags`) and identified as valid Opus by `ffprobe`,
  neither of which this crate had any part in. The decoded PCM is a genuine
  signal (peak/RMS/nonzero-fraction all sane), not silence or garbage.
  14/14 tests pass with `--features encode`; the original 9 still pass with
  no features, and pulling in zero libopus dependency, confirmed by reading
  the dependency tree rather than assuming the feature gate works.

  One packet per Ogg page (correct, not maximally size-efficient — a real
  encoder packs several small packets per page). Left as a documented
  follow-up in `encode.rs`'s own module doc rather than done now.
* A pack container saves 22% on blips alone but only 5.1% across a realistic
  bank, and costs a bespoke demuxer.
  `assets::the_pack_is_not_yet_worth_a_bespoke_demuxer` asserts the 10% line so
  the decision reopens by failing.

---

## 5. STORY — decisions are in `docs/films/CANON_DECISIONS.md`

That file is the record and says "settled unless marked OPEN". Built this
session from it: `link.rs` (the Watch's tether and the fare home),
`superform.rs` (super bodies DERIVED, not authored), `fragment.rs` (Fragments
of the Lattice), and the Lattice as **boss 20** (`the_anvil`, SYNC, arena
`unending`).

Two design failures worth knowing:

* **The obvious super-body transformation flattens the roster.** "Taller and
  broader, breadth faster than height" scored **-0.98** on recognisability —
  super Ice read as Fire — because BREADTH IS THE AXIS THE ROSTER IS SPREAD
  ALONG and `profile` is height-normalised. The fix separates uniform growth
  (`height_m` alone, which costs no identity) from amplified nature (each knob
  pushed further FROM the ice reference). A fragment should make a body more
  itself.
* **Four tests hardcoded the boss count and only one should have.**
  `raidcon`'s is the right home for the literal; the others now derive from
  `BOSSES.len()`, so a roster change is acknowledged in exactly one place.

Still OPEN in canon: the ledger (specificity loss — must cost DETAIL, never
power), the thermal gradient, a fourth `Medium` for a finding that is GIVEN
rather than found.

---

## 6. MODULES WITH NO CALLER

Honest gaps, not forgotten ones.

* **`suitability.rs`** — environment-driven vehicle choice, fully tested, no
  consumer, because no vehicle-selection surface exists. Wiring it now would
  mean inventing the caller.
* `linebank::verify` is called only by tests; the real caller needs
  `voiceprint-kit` wired to actual audio.
* **`blackstar-rs/src/rig.rs`** (found via `orphan_check.py`, then read rather
  than assumed) — no `mod rig` anywhere, zero tests, 445 lines. Not a stub:
  the doc comment cites measured stats from a real animation (Keelwright
  "Change form", 83 joints, 8.33s — 61% skinned, 39% rigid, the exact ratio
  that argues for walking the node hierarchy rather than a flat matrix list).
  This is proper glTF skin+clip evaluation — arbitrary continuous sampling of
  an authored animation.
  Two open questions, same shape as §9's `unendlich-world`:
  1. `xform.rs`'s morph is a DIFFERENT approach already shipping — vertex-lerp
     between 5 authored poses (`BLEND_SEC = 1.3`, tuned easing per damage
     state). It's not obviously the same problem `rig.rs` solves; `rig.rs`
     generalises to any glTF clip, `xform.rs` is hand-tuned for exactly one
     move. Whether `rig.rs` was meant to REPLACE that move's morph or ADD
     support for other animations `xform.rs` was never built to handle is a
     product question, not a technical one.
  2. `joints.rs` already reads a SEPARATE joint-position format written by an
     external `foldview --joints-json` tool, for debug visualisation only —
     not the same data path `rig.rs` would need (glTF skins/clips), but worth
     knowing about before assuming `rig.rs` is the only joint-aware code here.
  Not wired here: same reasoning as §9 — restoring a 445-line, zero-test
  animation evaluator into a shipping game blind, with no way to see the
  result in this sandbox, is how a "verified" claim goes wrong.

`link.rs` DID get a caller: `finale.rs`'s Photon climax now refuses off the
tether, which is the one place in six films the ceiling is reached.

---

## 7. THE HABIT THAT FOUND THE MOST BUGS

Nearly every real defect this session came from **a test written to assert a
design claim rather than a threshold**.

* Comparing gradient normals against face-averaged ones (to justify the
  choice) found that the triangle winding was inverted — the world would have
  rendered inside out, and watertightness, seam closure and vertex placement
  are all winding-agnostic, so nine passing tests said nothing.
* Asserting the `Bag` sequence differs between seeds found that `seed | 1`
  collapses ADJACENT seeds, so callers seeded from an incrementing id all
  heard the same sequence.
* Running the screenplay parser on the REAL files found seven speakers for five
  actors, because `**THE MORPHER (V.O.)**` is the same person. The fixture was
  correct and complete and missed it, because it was written from the same
  mental model as the parser.

The corollary: **when a test fails at a number close to the assertion, find out
why before loosening it.** The normals test failing at 0.9903 against 0.995 was
discretisation error — but loosening it to 0.98 would have hidden the winding
bug. The envelope test failing at 500 vs 502 was integer-hop drift worth
documenting, not smoothing over.
