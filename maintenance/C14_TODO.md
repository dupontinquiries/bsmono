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
   dandere2x. 15-30×, and `.jpg` is what a user expects a photo to be. Changes
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
