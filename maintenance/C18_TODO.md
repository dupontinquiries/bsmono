# C18_TODO — session c18unified, 2026-09-13

Next in the series after `C14_TODO.md`. Everything here was measured in this
session; where a conclusion rests on reading rather than running, it says so.

Fixed this session, so not repeated below: three stale lockfiles
(`7ecdff819`, `94fd05475` — `lock_check.py` now reports **stale: 0** across
143 lockfiles), and the rusty-board-egui focus hole (`3ee38b6b8`,
`05c7145b3`).

---

## 1. The daemon tier needs 1 of its 40 crates — C14 §16's open question, answered

C14 §16 worked out the right shape for `system/snapclone` and stopped at the
one thing it could not name: *"the crates those two binaries actually need"*.
That is computable, and the answer is smaller than the entry assumes.

`system/snapclone/systemd/` ships exactly two units:

    snapclone-bus.service            ExecStart=…/snapclone-bus
    snapclone-camera-bridge.service  ExecStart=…/camera-bridge

Walking `path =` dependencies from each:

| binary | produced by | in-tree crates in its closure |
|---|---|---|
| `snapclone-bus` | `snapclone-databus` | **1** (itself) |
| `camera-bridge` | `camera-bridge` | **0** (self-contained) |

`snapclone-databus`'s dependency list is anyhow, chrono, serde, serde_json,
tokio, tracing, tracing-subscriber, uuid. No path dependencies at all.

So the daemon tier carries, and does not need:

* **39 of 40 Rust crates** (2.4 MB, of which databus is 76 KB)
* **82 Qt `.cpp` UI files** plus `include/` — 4.6 MB of C++ and headers
* a 34 KB `CMakeLists.txt`, resources, packaging, a flatpak manifest

This does not change §16's conclusion — **do not archive the tier**, it is an
architectural layer and the operator said so. It sharpens the refactor: the
"everything else comes out" list is now enumerable rather than estimated, and
the part that stays is one crate and one self-contained bridge.

**Verified by:** parsing all 40 manifests for `path =` deps and taking the
closure; reading `snapclone-databus/Cargo.toml` directly to confirm the empty
result was real and not a parse miss. NOT verified by building either daemon —
no Qt6 or gstreamer host here.

### The two trees, re-measured

| | apps/snapclone | system/snapclone |
|---|---|---|
| files | 793 | 654 |
| byte-identical to the other | 597 (4.9 MB) | — |
| differing | 55 | 55 |
| only here | 141 | **2** |
| last commit | 2026-09-12 | 2026-08-22 |
| commits in the last 200 touching it | 200 | 8 |

§16 measured 784/654/132; the counts have moved slightly, the shape has not.

`system/snapclone/log.txt` is a **1.6 MB committed log file**. It is one of the
two files unique to that tree. Nothing references it. Deleting it is
independent of the whole refactor above and needs no decision.

## 2. Eight independent readers of `/sys/bus/iio/devices`, and the kit is not the one you'd reach for

| lines | file |
|---|---|
| 2034 | `shared/uikit/crates/uikit-egui/src/glove.rs` |
| 949 | `apps/voiceterm/egui-frontend/src/iio_accel.rs` |
| 434 | `apps/voiceterm/egui-frontend/src/iio_light.rs` |
| 379 | `games/blackstar-rs/src/gyro.rs` |
| 379 | `games/skydribble/src/gyro.rs` (byte-identical to the above) |
| 279 | `apps/snapclone/egui-frontend/src/glove_input.rs` |
| 203 | `apps/ambient-learner/src/environment.rs` |
| — | plus `systems/environment_system.rs` |

Three checked by hand independently glob `/sys/bus/iio/devices` and parse
`*_raw` / `*_scale`; voiceterm's also handles `in_accel_mount_matrix`, which is
the part everyone else will get wrong on a rotated phone panel.

**The trap, and I nearly walked into it.** `shared/sensor-kit` exists, mentions
IIO, and has **zero consumers** — `shared_survey.py` lists it among ten
substantial crates no port uses. The obvious writeup is "eight apps ignored the
kit". Reading it says otherwise: sensor-kit parses `iio-sensor-proxy`'s
`monitor-sensor` **text output** into coarse `Tilt(Up/Down/Normal)` and
`Orient(...)` events, and its own doc comment scopes it deliberately — *"the
platform reader … feeds lines/events in; the control math lives here"*. The
eight readers want continuous raw values from sysfs. Different layer.

So the finding is not "use sensor-kit". It is that **the source layer
sensor-kit explicitly excludes has been written eight times**, and the kit that
would hold it does not exist. That is also why sensor-kit has no consumers: an
app that needs continuous tilt cannot start from it.

Shape: an `iio` source module — device discovery, `_raw`/`_scale` parsing,
mount-matrix application — that sensor-kit can consume as one of its readers
and the games/apps can use directly. voiceterm's `iio_accel.rs` is the most
complete starting point of the eight.

## 3. The compile-time fixes in COMPILE_TIME.md were applied to one crate each

Both levers that file documents are real, measured, and were never propagated.

**Backend split.** COMPILE_TIME.md: *"Twenty-four crates in this repo build
both the x11 and wayland backends. A machine runs one."* The fix
(`app-x11` / `app-wayland` features, default unchanged) exists in exactly
**one** manifest — `apps/voiceterm/egui-frontend`. The count is now **37**
declarations enabling both. Measured saving there was 5 s per clean build, not
the 15 s originally projected, because winit still pulls part of x11rb
transitively; propagating it is a small win repeated 36 times, and more
importantly it is a real choice a phone target should have.

**Dev profile.** COMPILE_TIME.md's second lever: `[profile.dev] opt-level = 1`
governs workspace members, which are exactly the code you recompile on every
touch, while dependencies — compiled once and cached — sit wherever
`[profile.dev.package."*"]` puts them. Own code wants 0, dependencies want 2.

Of 30 manifests with a dev profile, **5** set the dependency override at all
(voiceterm, cadscope, unendlich, echelon/client, clawbound-battleground), and
**17** have own code at `opt-level > 0`:

    own=1: aperture, depot, drivemind, gitmind, rusty-board,
           rusty-board/egui-frontend, snapclone/rust, tomescope,
           system/snapclone/rust, shared/uikit, clawbound,
           echelon/client, unendlich
    own=2: cadscope, simscope, blackstar-rs, skydribble

Only voiceterm has the full corrected pair (own 0, deps 2).

Three of the 17 are deliberate and must not be blanket-edited. `cadscope` sets
own=2 / deps=0 on purpose — its comment explains that wgpu-core and naga at
opt 2 blow the inner-loop budget while the `cad-*` crates need speed, and that
works because `[profile.dev.package."*"]` excludes workspace members.
`skydribble` and `blackstar-rs` set 2 with a comment that physics and png
decode at opt 0 are unusable; they want the *split*, not the removal.

**Measuring-instrument note.** My first pass reported voiceterm as own=1, one
of the offenders. It is 0. The regex had matched `opt-level = 1` inside
voiceterm's own comment *explaining why 1 was wrong*. The numbers above come
from a re-run with comments stripped. Worth stating because the wrong version
was internally consistent and I would have quoted it.

**Unmeasured.** No before/after timing on any of the 14 real candidates.
COMPILE_TIME.md's one projection was wrong by 3x when measured, so time one
crate before touching the rest.

## 4. Duplication outside the snapclone fork is four sets, and three are one pair of games

Hashed every `.rs` over 1.2 KB in the tree. 154 duplicate sets; all but four
are the `apps/` ↔ `system/` snapclone fork (§1).

The remainder:

* `games/blackstar-rs/src/{fx,gyro,platform}.rs` are **byte-identical** to
  `games/skydribble/src/{fx,gyro,platform}.rs` — 32 KB across three files.
  `games/shared/` already exists as a home (bsweb, isosurface, linebank,
  netcode, perfbisect, shellkit, worldhost). `gyro.rs` should go wherever §2
  lands instead; `fx.rs` (fixed-capacity particle ring buffer) and
  `platform.rs` (FP5 detection) are straightforward `games/shared/` candidates.
* `tools/rustcheck/rustcheck.rs` ↔
  `apps/logscope/reference-docs/rustcheck-tool/rustcheck.rs`, 45 KB. The second
  is under `reference-docs/`, so this is probably a deliberate snapshot — but
  LAYOUT.md records rustcheck as already de-duplicated to one source copy, and
  a byte-identical 45 KB file at a second path is what that de-duplication was
  supposed to have removed. Worth one look to decide snapshot vs leftover.

The headline is the negative result: **outside one known fork, this tree is
genuinely well de-duplicated.** Four sets in 4,760 files.

## 5. Orphaned modules: six, carrying no tests

`orphan_check.py` after the merges: `gpu-gemm/src/kernels.rs` (339),
`blackstar-rs/src/rig.rs` (446), `unendlich-engine/src/render/bloom.rs` (691)
and `postfx.rs` (295), and `shared_frames.rs` (13) in **both** snapclone
camera-bridges.

All predate this session's merges — checked with `git log` per file. Unlike the
nineteen orphans CLAUDE.md §7 records, these carry **0 tests**, so nothing is
silently not running. `bloom.rs` at 691 lines is the one worth a decision:
wire it or archive it per §7b.

## 6. A checker's own comment nearly cost a real bug

Recorded because the near-miss is more useful than the fix.

`lock_check.py` reported `worldhost-unendlich` as stale for a transitive
`voxel-tools`. Three things argued false positive: the crate reaches
`unendlich-engine` with `optional = true` **and** `default-features = false`;
the checker only skips optional path deps *past the first hop*
(`if not with_dev and OPTIONAL.search(...)`), so a first-hop optional dep is
still followed; and the function's own comment names this exact crate as the
source of a previous spurious `egui` report.

It was a real stale lock. `unendlich-engine` **is** in the lock and
`voxel-tools` is not, and voxel-tools is a plain non-optional dependency of the
engine — so the absence was staleness, not a feature the resolve had skipped.
Settled by reading the lockfile, not by reasoning about the checker.

The generalisable bit: a checker with a documented false-positive class will
launder a true positive of the same shape. The first-hop asymmetry in
`path_deps` is still there and is worth either fixing or explaining.

---

# Second pass

## 7. The near-duplicate band is where the risk is, and it is a different list

§4's byte-identical scan found four sets outside the snapclone fork and read
as "this tree is clean". That was the wrong instrument for the question.
Byte-identical copies have not drifted *yet*; the ones that have are invisible
to a hash.

Re-scanned comparing normalised line SETS (comments and blanks stripped) for
same-named files, reporting the 55–99% band. 27 pairs. The band matters:
100% is §4's list, and unrelated files never reach 55%.

Ranked by lines at risk:

| lines | pair | similarity |
|---|---|---|
| 19070 | `games/blackstar-rs/src` ↔ `games/skydribble/src`, 15 modules | 55–100% |
| 2421 | `bo-suite/bo-mux` ↔ `bo-suite/bo-controller`, 4 modules | 69–99% |
| 2233 | `rusty-board/src/gestures` ↔ `crates/board-gestures`, 2 modules | 95–97% |
| 150 | `voiceterm/src/whisper_pool.rs` ↔ `logscope-egui/src/whisper_pool.rs` | 89% |
| 443 | `bot-arena/src/fsm.rs` ↔ `unendlich/docs/.../reference/fsm.rs` | 99% |

The rusty-board pair is now guarded (`board_gestures_parity_check.py`,
`2f63ae549`). The rest are below.

## 8. blackstar-rs and skydribble are one forked engine — the largest kit candidate outside snapclone

Fifteen same-named modules, **19,070 normalised lines** on the blackstar side:

    100.0%   346  fx.rs          92.7%  4033  world.rs
    100.0%   299  gyro.rs        89.7%   804  meshgen.rs
    100.0%    78  platform.rs    87.2%   711  bots.rs
     98.9%    99  font.rs        82.7%  1111  render.rs
     96.8%   259  partcars.rs    82.2%  6338  sim.rs
                                 81.2%   680  procars.rs
                                 72.2%   409  config.rs
                                 68.6%  1920  physics.rs
                                 59.6%   515  sound.rs
                                 55.5%  1468  hud.rs

`games/shared/` already exists (bsweb, isosurface, linebank, netcode,
perfbisect, shellkit, worldhost). The three at 100% (`fx`, `gyro`, `platform`,
723 lines) can move today with no reconciliation; `gyro.rs` should go wherever
§2's IIO source layer lands rather than into a game crate.

`world.rs` at 92.7% of 4033 lines and `sim.rs` at 82.2% of 6338 are not a
tidy-up — they are a decision about whether the two games share an engine.
Worth answering before either grows further, but not a commit.

## 9. bo-suite duplicates 2421 lines of DSP between two crates in ONE app, and already has the dependency

The easiest of the three by a distance, and the one I would do first if it
could be compiled here.

    99.2%  1209  passby_canceller.rs
    90.6%   407  masker.rs
    72.3%   666  drone_canceller.rs
    69.4%   139  cancel_tones.rs

Both crates are members of the same workspace. `bo-mux/src/lib.rs` already
declares `pub mod passby_canceller;`. `bo-controller/Cargo.toml` **already
depends on bo-mux** — added for `mux_bridge.rs`, with a comment noting the
dependency had been missing. And `bo-controller/src/main.rs` declares its own
`mod passby_canceller;` beside it.

So the fix for the 1209-line file is: delete
`bo-controller/src/passby_canceller.rs`, swap `mod passby_canceller;` for
`use bo_mux::passby_canceller;`, done. Nothing to reconcile either — the only
difference between the two copies is that bo-mux's carries an extra test
assertion (that the FSM leaves Peak rather than stalling). bo-mux's copy is a
strict superset.

`masker.rs` at 90.6% needs the diff read. `drone_canceller` and `cancel_tones`
have genuinely diverged and are separate work.

**Not done here because bo-controller declares `qmetaobject = "0.2"` and Qt6
is not installable in this sandbox** (CLAUDE.md §8). An unverifiable edit to
the module wiring of a binary I cannot compile is exactly what that rule is
about. Two comments in the same crate —
`drone_canceller.rs:457` ("Shared helpers (mirrored from passby_canceller for
now)") and `frequency_shifter.rs:81` ("inlined here so…") — say a third layer
of the same duplication is already known and accepted.

## 10. Dependency version fragmentation: one free win taken, five real ports left

`fleet_build.sh` compiles every app into ONE shared target directory, so a
dependency resolved to N versions is N compilations in the fleet build.

Counting resolved versions across all 143 lockfiles, the wgpu family (wgpu,
wgpu-core, wgpu-hal, naga, wgpu-types) was **30 distinct compilation units**.

Five of those were pure lockfile drift — four workspaces on 29.0.3 against
thirty-two on 29.0.4, both declaring `wgpu = "29"`, nothing pinned. Unified in
`69bc263f1`: **30 → 25**, no manifest edit, no code change, gpu-gemm verified
building against it.

What remains is real and each needs a port:

| wgpu | workspaces |
|---|---|
| 29.0.4 | 32 — the mainstream |
| 22.1.0 | blackstar-rs, clawbound, skydribble, soldier, snapclone/camera-bridge |
| 0.19.4 | fao-codec, blackstar/foldview, **clawbound-battleground**, road-warrior |
| 24.0.5 | echelon/client |
| 27.0.1 | beatspace |

**`clawbound-battleground` is on wgpu 0.19 and glam 0.27** — the oldest in the
tree — while its sibling `games/clawbound` is on wgpu 22 and everything else
on glam 0.28. That crate came in with this session's merge (`5adbcae1e`), so
the merge added the tree's most out-of-date GPU stack. Worth a look before it
sets.

egui is in better shape: 44 declarations on 0.34, with 0.27 (×2), 0.28 and
0.29 outstanding. Two of those four are already recorded as
`font_install_check.py` known misses BLOCKED ON A VERSION BUMP, so the version
spread and the boxed-glyph bug are the same problem.

Also checked and NOT a bug: `games/echelon` pins `serde = "=1.0.203"` exactly,
which looks like the sort of pin that forces a duplicate build. Its comment
says it is deliberate, for distro rustc 1.75, and names `apps/bot-arena` as
the same convention.

## 11. Fixed this pass

* `card-diagnoser` never called `uikit_egui::fonts::install`, so the arrows in
  `reader.rs` rendered as boxes (`4fe8b6145`). It already had the uikit-egui
  dependency, so unlike the switcheroo case the checker was written for, this
  was a one-liner. `font_install_check.py` now reports no MISSING.
* wgpu 29.0.3 → 29.0.4, above.
* `board_gestures_parity_check.py`, §7.

Clean runs after all of it: `lock_check` stale 0/143, `eframe_backend_check`
all explicit, `aarch64_flags_check` clean, `archive_check` complete provenance,
`voiceprint_parity_check` PASS, `board_gestures_parity_check` PASS.

## 12. Two crates gate real code on features that do not exist

`cfg_feature_check.py` (new) scans every crate's `src/` for
`#[cfg(feature = "x")]` and `cfg!(feature = "x")` and checks `x` against the
manifest's `[features]` plus its optional dependencies. Two hits in 321 crates,
and both are live bugs rather than tidiness.

### `apps/rusty-board` — `llm-swipe-ffi`, 9 sites

`src/gestures/llm_swipe.rs` (455 lines) gates its entire real implementation on
a feature `Cargo.toml` does not declare. Confirmed by resolution, not by
reading: `cargo tree --features llm-swipe-ffi` gives
`error: the package 'rusty-board' does not contain this feature`, while
`--features llm` resolves fine.

Consequences, compounding:

* the real `RerankerInner` — the one holding a loaded GGUF model — has never
  been compiled, so it has never been type-checked. It references
  `crate::llm_backend::{LoadedModel, GenParams, load_model, generate}`, and
  **there is no `llm_backend` module in rusty-board**; the crate has `src/llm/`
  instead. The only `llm_backend` in the tree is logscope's, so this was
  adapted from there and never rewired. It could not compile even if the
  feature were declared.
* the `cfg(not(...))` mock arm therefore compiles unconditionally and is
  permanently the real path.
* `RerankerError::NotCompiledIn` reads "build with --features llm-swipe-ffi",
  which is an instruction that errors out.
* nothing outside the file references `LlmSwipeReranker` — grepped across the
  whole repo, per CLAUDE.md §8, not just the one directory.
* three docs name the feature three different ways: `llm-swipe-ffi` in the
  code, `llama-ffi` and `llm` in `docs/QT_REWRITE_HANDOFF.md`. None exists.

`config.rs` still exposes `llm_swipe_enabled` and `llm_swipe_model_path` as
user settings, so the config surface advertises a feature that is mock-only.

This is unfinished work, not superseded work, so §7b's archive path is not
obviously right either — deleting it loses a design, declaring the feature
hands the next person a build error. It needs a decision, and it is 455 lines
of the swipe path nobody can currently reach.

### `games/unendlich/crates/voxel-tools/harness` — `ai-engine` / `ai-gpu`, 19 sites

The harness crate has an EMPTY `[features]` section, and the parent
`voxel-tools` does not declare these either. `paintbrush.rs` branches on both,
including a `cfg(all(feature = "ai-gpu", not(feature = "ai-engine")))` that can
never be true. Not read in depth; recorded with the same shape as above.

### Note on the checker

rustc's `unexpected_cfgs` lint covers part of this since 1.80, but it is a
warning among many and it says nothing about a crate nobody currently
compiles — which is the state both of these are in.

One false positive was found and designed out rather than tolerated:
`apps/voiceterm/src/build_diagnostics.rs` embeds captured compiler output as a
string literal containing `#[cfg(feature = "winit")]`. The checker only counts
an attribute that opens a line, or the `cfg!()` macro. Verified by injecting
that exact string-literal shape into a clean crate and confirming it is not
flagged, alongside three positive break tests (declare the missing feature →
that crate drops off; inject a phantom cfg into crypto-kit → caught by name).

---

## 13. logscope-egui and the voice popup: one port, two consumers, currently zero

Scoped after being asked for "logscope-egui's system log pages, and
voiceterm-qt's voice popup alongside voiceterm-egui". Surveyed before
building, and the survey changed the shape of both asks.

### The system-log pages are not missing

`logscope-egui/src/*.rs` lacks 122 modules that `logscope/src/*.rs` has,
which looks damning until you read `logscope-egui/Cargo.toml`:

    logscope = { path = "../logscope", default-features = false }

The egui crate DEPENDS on the GTK crate as a headless library, GTK gated
off. The 122 modules are consumed, not absent — the manifest comment says
"most of its modules have since diverged from logscope's originals", which
is a statement about drift, not about coverage. logscope-egui is also the
LARGER crate (192 files against 149).

`journalctl` handling specifically lives in `diagreport.rs` (folding a dump
into a report's ERRORS section as bursts) and `externalwatch.rs`, both in
the shared headless crate and both reachable from the port.

So "functioning pages for system log viewing" needs a NAMED page that is
wrong, not a file-count comparison. Nothing was changed here on the strength
of one.

### The voice popup exists in exactly one place, and it is QML

`voiceterm/src/voice.rs` is 4,417 lines and **entirely Qt-free** — zero
`qmetaobject`/`QString`/`QObject` references, and zero `use crate::` lines,
so it is self-contained. What is Qt-bound is only the SURFACE: the popup
itself is QML, and voice.rs's comments describe pushing events "for QML to
consume".

The important finding is that **voiceterm-egui does not have the popup
either**. Grepping its frontend for voice turns up `voice_prefix`,
`voice_routing` and `repl_voice_glow` — prefix parsing, routing and a REPL
glow effect — but no capture-and-preview popup. And logscope-egui's "voice"
hits are a MODEL-SELECTOR category (Large/Small/Voice/Image), not speech.

So the ask is one port with two consumers and no existing implementation on
either side. That is a shared-kit shape, not a copy-into-logscope shape:
mounting voice.rs twice by `#[path]` would put the wake-word state,
partial-preview buffer and recording lifecycle in two places, and voice.rs
keeps that state in module-level statics (`cancel_recording()`,
`get_partial_preview()`, `clear_partial_preview()` take no receiver), so two
mounts would be two independent recorders fighting over one microphone.

### Shape, in dependency order — CORRECTED: `shared/voice-kit` already exists

Step 1 below originally said "`shared/voice-kit` (or into an existing kit)
taking voice.rs as its engine." Checked before starting the extraction, and
that sentence was wrong: `shared/voice-kit` already exists, and `voice.rs`
already consumes it —

    voice_kit::hallucination::looks_like_speech(text)
    voice_kit::hallucination::trim_hallucination_loop(s)

— for exactly what its own manifest says it is for: "Shared whisper-voice
post-processing + model resolution... Zero-dependency." Two comments in
`voice.rs` (near line 1301 and 3586) record this as a prior repoint, not
something to redo.

So `voice-kit` is the WRONG target for this port. It is deliberately narrow
— text-level hallucination filtering and model resolution, explicitly not
"the signal-level math" (that is `audio-kit`) and not the recording
lifecycle, wake-word state or popup surface `voice.rs` also holds. Taking
the whole 4,417-line file into `voice-kit` would bloat a zero-dependency
text-processing crate with `cpal`/`whisper-rs` recording plumbing it was
explicitly written to exclude — the opposite mistake from not extracting at
all, and the kind of over-broad grab this session has been undoing rather
than committing (`av1tuner`'s six `crate::X`-should-be-`super::X` paths and
`bo-suite`'s two dedups were both about respecting an existing boundary,
not inventing a bigger one).

### The real popup surface, read from every call site rather than guessed

Corrected again, this time by doing the read the previous version of this
section deferred. `voice::` is called 77 times from `main.rs`, across 32
distinct functions — not the "~8-10" guessed earlier from function names
alone. Grepping `voice::FN` gave the full list; reading each call site's
surrounding QObject method (the actual QML binding, not the Rust name)
separated it into two real groups.

**The popup surface — 10 functions, all clustered around the QML-facing
methods `voice_listen`, `voice_begin_capture`, `voice_cancel`,
`voice_finish`, `voice_listen_dual`, `voice_partial_preview[_words]`,
`voice_pop_wake_event`, `voice_set_wake_suppressed`,
`voice_is_wake_suppressed`:**

    begin_capture              cancel_recording         finish_recording
    listen                     listen_with_audio
    get_partial_preview        get_partial_preview_words
    pop_wake_event
    set_wake_suppressed        is_wake_suppressed

The doc comments on the QObject methods ARE the popup contract, verified
rather than inferred: `voice_set_wake_suppressed`'s comment says "QML
controls wake suppression — set true on popup open, false on close.
Prevents a wake firing mid-foreground-utterance" (confirming the wake-
suppression rule this section already named from pty_term.rs/main.rs
independently), and `voice_pop_wake_event`'s says the captured audio is
stashed for "the next voice flow (which QML triggers in response)" —
naming the exact hand-off a shared popup has to reproduce.

**Correction, found while doing the 1a extraction itself:**
`set_force_tiny_tier` and `set_fast_finalize` were misfiled below as
"tuning knobs, stays behind." Both are read INSIDE the core's own call
chain — `FORCE_TINY_TIER` in `transcribe_with_dual_model` (line 3743),
`FAST_FINALIZE` in `record_and_transcribe_inner` (line 3185) — and their
own doc comments say they are "set ONLY by QML paths that know the next
utterance will be short," i.e. genuinely popup-facing one-shot hints, not
engine configuration. They belong in 1b (popup surface) alongside their
statics, not in the "stays in voiceterm" list. This is exactly the kind of
correction a name-based guess cannot catch and a trace does: both looked
like config from their names alone.

**Stays in voiceterm, not part of a shared surface — the other 20 (was
22; corrected above):** tuning knobs (`set_whisper_interval_ms`,
`set_phonetic_interval_ms`), device/model plumbing
(`input_device_names`, `prewarm`, `warmup`, `is_available`), wake-phrase
persistence (`registered_wake_phrase`, `set_registered_wake_phrase`,
`load_registered_wake_phrase_from_disk`), and diagnostics
(`wake_bursts_detected`, `wake_fired_count`, `wake_last_rms`,
`pop_bg_transcription`, `pop_ask_ai_event`). These configure or introspect
the engine; a popup does not need to call them to draw itself, and several
(the wake-phrase setters) are settings-page concerns in the Qt build, not
popup concerns.

The remaining design question — worth stating rather than deciding here —
is whether `pop_wake_event`'s `last_audio_buffer` hand-off (currently: the
QObject stashes it as a field, and a SEPARATE QML-triggered call reads it
back out) can cross a plain function-call shared surface at all, or whether
it needs an explicit return value instead of an implicit stash-and-fetch.
That is a real API decision for whoever builds the surface, not something
to default without picking.

Dependency order, step 1 now split into 1a/1b now that the layers are
confirmed rather than assumed; steps 2-4 unchanged:

1a. Extract the shared core FIRST, on its own, with no popup-shaped API:
    `ensure_models_loaded`, `load_tier`, `transcribe_with_dual_model`,
    `run_one`, and the `*_CTX`/`*_STATE` statics. This is the piece both
    `warmup`/`prewarm` and the popup surface need, so it has to exist
    before either side is cut over — cutting the popup surface out first
    would immediately hit the same "which crate owns TINY_CTX" question
    this section spent its last two passes on.
1b. Then the popup surface proper (`RING_BUFFER`, `CANCEL_FLAG`,
    `FINISH_FLAG`, the preview/wake-event statics, the 10 functions),
    depending on 1a's crate rather than reaching into voiceterm.
    `listen_with_audio`'s reach into `crate::log_summary::VOICE_ACTIVE`
    needs a decision here: a callback the caller supplies, or leave that
    one flag as a voiceterm-side effect set around the call rather than
    inside it.
    `voice.rs` becomes a thin voiceterm-specific wrapper over 1a+1b, the
    way `av1tuner`'s `gm` module's children now correctly point `super::`
    at their real parent instead of `crate::`.
2. An egui popup surface over 1b, in a shared widget crate, since both
   consumers want the same one. voiceterm's QML is the reference for what it
   shows: partial preview with per-speaker colouring, cancel/finish, and the
   wake-suppression rule (`voice popup open` must block a wake firing
   mid-utterance, per pty_term.rs:2538 and main.rs:2850).
3. Mount in voiceterm-egui first — it is the app whose whole premise is
   voice, and the one with the QML to compare against.
4. Then logscope-egui, which is the case with no reference implementation
   and should not go first.

`filler_words.rs` also records that a cancelled popup feeds the filler-word
corpus ("voice popups the user cancelled without executing"), so the cancel
path has a consumer and is not merely a dismiss.

**Still not started, and now genuinely ready to be.** Three passes in this
session went into scoping (wrong target, then an unverified function list,
then a suspected coupling) and this one confirmed the coupling by tracing
real call chains rather than reasoning about them. The shape is now known
precisely enough that step 1a is a mechanical extraction — pick the
functions and statics named above, move them, re-export from `voice.rs` —
rather than a design question. That mechanical step is what the next
session should spend its budget on, not more scoping.

**Traced, and the coupling is real — confirmed, not suspected.** Read
`listen()` and `listen_with_audio()`'s full call chains against all 27
statics rather than stopping at their own bodies, since a static touched
three calls deep is invisible to a shallow read.

    listen() -> listen_with_audio() -> record_and_transcribe_inner()
             -> ensure_models_loaded() / transcribe_with_dual_model()
             -> touch TINY_CTX, BASE_CTX, LARGE_CTX (and load_tier(),
                run_one() touch the matching *_STATE pair)

`record_and_transcribe_inner` itself only touches `RING_BUFFER`/
`CANCEL_FLAG`/`FINISH_FLAG` directly — the model statics are two calls
further in, which a shallow read of the popup-facing function alone would
have missed entirely. Then checked the three functions scoped as
"config, stays behind" against the SAME chain, expecting them to be
independent:

    warmup()   -> transcribe_with_dual_model() -> same TINY/BASE/LARGE_CTX
    prewarm()  -> ensure_whisper_loaded() -> ensure_models_loaded() -> same
    is_available() -> touches NONE of the 27 statics (file-exists + `which`
                       checks only) -- genuinely independent, confirmed

They are not independent. `warmup` and `prewarm` reach the identical model
statics through one-line wrappers (`ensure_whisper_loaded` is a bare
pass-through to `ensure_models_loaded`) around the exact same loader
`listen`'s chain calls. There is no separate "config-only" path at the
model layer — everything that ever transcribes or warms the model funnels
through one `ensure_models_loaded`/`load_tier` pair. `is_available` is the
one of the three that is genuinely independent, confirmed rather than
assumed by the same check.

So the shape is not "10 popup functions in one crate, 22 engine functions
in another." It is three layers: a shared model-loading/inference core
(`ensure_models_loaded`, `load_tier`, `transcribe_with_dual_model`,
`run_one`, the `*_CTX`/`*_STATE` statics) that BOTH the popup surface and
`warmup`/`prewarm` need; the popup surface proper (`RING_BUFFER`,
`CANCEL_FLAG`, `FINISH_FLAG`, the preview/wake-event statics, and the 10
functions built on them); and the handful of genuinely independent
functions like `is_available`. The core has to move wherever the popup
surface moves, or the popup crate depends on voiceterm for it — inverting
the extraction, which was the failure mode this whole investigation was
checking for.

### The core's full dependency closure — bounded, not the 13-module reach it looked like

`voice.rs` as a whole reaches into 13 sibling modules (`speakers`,
`event_signal`, `acoustic_index`, `wake_gate`, `voice_lang`, `separation`,
`phonetic`, `audio_profile`, `log_summary`, `whisper_pool`,
`corpus_recorder`, `security`, `peer_handshake`, plus `clog!` 98 times) —
which looked like it made the whole file too entangled to extract any
piece of cleanly. Checked what the SPECIFIC four core functions (1a's
target: `ensure_models_loaded`, `load_tier`, `transcribe_with_dual_model`,
`run_one`) actually touch, rather than assume they inherit the whole
file's footprint: only `crate::clog`, `crate::voice_lang`,
`crate::whisper_pool`.

Both `voice_lang.rs` (173 lines) and `whisper_pool.rs` (360 lines) have
**zero** `use crate::` themselves — self-contained, so they do not drag in
a second hop of the other 11 modules. `clog!` expands to
`$crate::log::emit(...)`, which resolves against whichever crate invokes
it — so the macro itself is portable to a new crate with its own `log`
module, or the shared crate can take an `emit` callback instead.

The core also needs `ModelTier`, `model_file`, `forced_tier`, `model_path`
(~80 lines, `voice.rs`-local) and the already-external `models_kit` crate
`model_file` calls into.

So the real closure for 1a is: 4 functions (~550 lines total) + 6 model
statics (`TINY_CTX`/`BASE_CTX`/`LARGE_CTX`/`TINY_STATE`/`BASE_STATE`/
`LARGE_STATE`) + 2 more found only by reading `transcribe_with_dual_model`
line by line rather than grepping static names against it —
`FORCE_TINY_TIER` and `FAST_FINALIZE`, one-shot popup hints read inside
the core's own call chain (line 3743 and, via `record_and_transcribe_
inner`, line 3185) — + `ModelTier`+3 helpers (~80 lines) + two small
self-contained sibling modules (533 lines) + the `clog!` macro (needs
`$crate::log::emit` in the new crate, or a callback parameter) +
`models_kit` (already shared). `run_one` takes
`&whisper_rs::WhisperContext` as a plain parameter rather than reading a
specific tier's static itself, so it does not add a dependency of its own
— its caller (`transcribe_with_dual_model`) is what selects which context
to pass in. `PREVIEW_STATE` was nearly counted into this list and does not
belong here: checked its one call site (`ensure_preview_thread`, line
1173) and it backs the LIVE-PREVIEW text path, which is a popup-facing
(1b) concern, not part of `listen()`'s call chain.

Their SETTER functions (`set_force_tiny_tier`, `set_fast_finalize`) do not
have to live in 1a's crate even though their statics do — a setter is a
one-line atomic store the popup surface (1b) can call across the crate
boundary on a `pub` static, the same way `enable_cmd`/`unit_path` in
gitmind's daemons module stayed thin wrappers over shared state earlier
this session. What matters is that the STATICS move with the core, since
that is what `transcribe_with_dual_model` reads.

Roughly 1,200 lines total, not the whole 4,417-line file and not an
unbounded dependency tangle — a normal-sized extraction, comparable to
`board_layouts::dispatch` earlier this session. The two hint flags found
just now are the reason this was read function-by-function rather than
grepped: a grep for known static names would have missed them entirely,
the same way the model-context coupling itself was invisible to a
call-site grep on `main.rs`.

**Correction, and a more serious one than the last two.** Continuing the
verbatim read into `record_and_transcribe_inner` itself (the ~417-line
function `listen_with_audio`'s chain calls, previously only grep-checked
against the known static list on a truncated extract) found real
dependencies that grep missed because they are FUNCTION CALLS, not static
names:

    crate::audio_profile::noise_floor()       -- itself pulls in
                                                  crate::phonetic::TonalProfile
    crate::acoustic_index::{promote_enabled, fingerprint,
        find_best_by_fingerprint, promote_thresholds, record_hit,
        record_shadow_observation, add_exemplar, Exemplar}
    crate::speakers::current_active_id()

`audio_profile` (707 lines) is NOT self-contained the way `voice_lang`/
`whisper_pool` are — it has its own `use crate::phonetic::TonalProfile`,
pulling in an eighth module. `acoustic_index` is a whole fingerprint-cache
subsystem (7 functions plus a public type), not a helper. `speakers` was
already known as a heavy dependency (13 uses across the file) but not
previously connected to this specific function.

This means the "~1,200 lines, bounded, comparable to
`board_layouts::dispatch`" estimate two commits ago was wrong, and wrong in
the way that matters: it under-counted by not reading
`record_and_transcribe_inner` past its static-name footprint. The Tier1
acoustic-index fast path, the speaker-aware fingerprint lookup, and the
noise-floor-adaptive VAD threshold are most of the function's body (lines
3350-3550 of ~3155-3572).

So 1a is not "the model-loading core," cleanly separable from everything
else, on the strength of the file structure alone. It reaches into
`audio_profile`, `phonetic`, `acoustic_index`, and `speakers` — four more
modules, none yet confirmed self-contained the way `voice_lang`/
`whisper_pool` were.

### The four checked — the closure is real, and would make the popup crate bigger than voice.rs

    audio_profile     707 lines   -- use crate::phonetic
    phonetic         1860 lines   -- leaf (std only)
    acoustic_index    997 lines   -- leaf (std only)
    speakers         1405 lines   -- use crate::phonetic

`phonetic` and `acoustic_index` are leaves — the closure stops here, it
does not keep unrolling — but it stops at roughly **4,969 lines** for
these four alone, more than the whole of `voice.rs` (4,417 lines). Taking
all of this into a "small popup surface" would mean the shared crate ends
up larger than the file the popup was meant to be extracted FROM.

### The resolution: the acoustic-index path is opt-in, not load-bearing

Before concluding the extraction is impractical, checked whether the Tier1
section is actually required for `listen()`'s DEFAULT behavior or only for
an optional enhancement — because if optional, the boundary can be drawn
BEFORE it rather than after, which is a much smaller extraction. (This
also corrects something said too quickly two paragraphs up: "skipping the
acoustic-index check would mean every consumer loses the latency win" was
written before this check, and is not quite right — see below.)

It is optional, in the function's own words: *"Default behavior (SHADOW
mode): no behavioral change. Whisper always runs."* The skip-Whisper fast
path only activates when `WEB2QML_TIER1_PROMOTE=1` is explicitly set — off
by default. Unset, the acoustic-index calls still run (for shadow-mode
logging: recording cache/Whisper agreement for later tuning) but nothing
they return changes what gets transcribed.

So a shared core does not need `acoustic_index`, `speakers`, or
`audio_profile`'s noise-floor read to reproduce voiceterm's DEFAULT voice
behavior. Those three are a voiceterm-specific enhancement layer sitting
on top of `listen()`/`transcribe_with_dual_model`, not something a popup
needs to function. The boundary: the shared crate implements plain VAD (a
simply-adaptive threshold, no acoustic-index lookup, no fingerprint cache)
and voiceterm's own wrapper re-adds the Tier1/speaker layer as a
voiceterm-specific pre-check in front of the shared `transcribe`, not
inside it.

This does mean the shared crate's VAD will not be bit-identical to
voiceterm's CURRENT behavior even with `WEB2QML_TIER1_PROMOTE` unset:
losing the noise-floor-adaptive threshold (the `audio_profile::
noise_floor()` read) is a real difference for voiceterm specifically, even
though shadow mode changes nothing a user sees today. Whether voiceterm
keeps a richer VAD as a wrapper, or the shared crate's VAD gets enhanced to
match once `audio_profile`'s further dependents (if any beyond `phonetic`)
are confirmed bounded, is a real design choice for whoever builds this —
stated as a choice now rather than left as an undiscovered blocker.

With that boundary, the ACTUAL 1a closure returns to roughly the original
size: `ensure_models_loaded`, `load_tier`, a plain-VAD version of
`record_and_transcribe_inner` (its acoustic-index section removed, its
`RING_BUFFER`/`CANCEL_FLAG`/`FINISH_FLAG`/`FAST_FINALIZE`/
`FORCE_TINY_TIER` handling kept), `transcribe_with_dual_model`, `run_one`,
the 6+2 statics, `ModelTier`+3 helpers, `voice_lang`+`whisper_pool`
(confirmed self-contained), `clog!`. `audio_profile`, `phonetic`,
`acoustic_index`, `speakers` stay in voiceterm, feeding voiceterm's own
wrapper rather than the shared core — back to roughly 1,200-1,400 lines,
this time because the boundary was actually checked rather than assumed.

One more coupling, from `listen_with_audio` specifically — part of 1b (the
popup surface), not the four core functions above, so it does not block
1a. It reaches into `crate::log_summary::VOICE_ACTIVE`, a deferral flag
telling a log-summary worker to back off while voice has the CPU. A shared
popup crate needs an answer for this too: a callback, or leaving that one
flag as a voiceterm-side effect set around the call rather than inside it.

---

## 14. snapclone: the photo popup and call slide-up are already ported

Scoped after being asked for "the snapclone-qt take a photo popup for adding
text and choosing senders" and "the slideup for calls". Both exist. Recorded
so the ask is not re-opened on the strength of a filename search that misses
them.

### The photo popup

`SnapComposerDialog` (592 lines of C++) is ported as `composer.rs` (302) +
`composer_ui.rs` (184), and the port carries the parts that matter rather
than the widget tree: caption baked into the image before send (so the
recipient's client needs no overlay agreement), `cycleColor`, swipe-up-to-send
so the whole background is the button, and keyboard-inset avoidance.

The duration trap is documented in the port and worth repeating here:
`duration_secs = 0` means NO LIMIT on this wire, not "vanishes instantly", so
`if (duration)` is exactly backwards and leaks the photo. `send_flow.rs`
records that the egui bin was writing 0 for every snap before it was fixed.

Choosing recipients is the second half and is also there: `composer_ui.rs`
ends on "Next: choose recipients", and `send_flow.rs` restores the Qt
four-step sequence (take -> compose -> pick -> receipt) that the port
originally had "all the pieces and none of the sequence" of.

**One real gap, and it is on the QT side.** `SnapComposerDialog.cpp:41`:

    // caption() stub — Snap struct has no caption field yet.
    QString SnapComposerDialog::caption() const { return {}; }

The caption is baked into the pixels and never travels as text, so nothing
downstream can search, index or re-render it. That is a Snap-struct change,
not a port.

### The call slide-up

`IncomingCallDialog.cpp` (175 lines) is ported as `ring.rs` (518) — larger
than the original, because it also absorbed `RingtoneSynth` and the timeout
rule: Decline and the 30-second timeout are the same answer, since a call
nobody picked up was declined and the caller should not hear a ring that
never ends.

### What IS unported, measured rather than guessed

Of 82 Qt UI surfaces (`src/ui/**/*.cpp`), 65 are named somewhere in
`egui-frontend/src/`. The 17 that are not:

    CameraBridgeManager  CameraRegistry     CameraTester
    ComponentTestWindow  FocusSweep         FramePixelOps
    IioSensorProxySource LibcameraBackend   ModalLayerTestPanel
    PostureController    QtCameraBackend    RecordingMetadata
    VcmController        VideoRecorder      VideoTranscoder
    VoiceActivityTracker converter

Almost none is a screen. They are camera-backend plumbing (libcamera and Qt
backends, lens VCM control, focus sweep, pixel ops), video recording and
transcoding, two test harnesses, and `IioSensorProxySource` — which belongs
with C18_TODO section 2's IIO source layer rather than being ported into
snapclone a ninth time.

The user-facing port is complete. The gap is the camera BACKEND, which is a
different piece of work and wants deciding as one (does the egui build talk
to libcamera directly, or through camera-bridge?) rather than surface by
surface.

---

## 15. Two field reports from device testing (2026-09-13)

Both logged from real use. Both checked against the tree, and neither is
what the symptom suggests — recorded so the next person does not start by
writing the thing that already exists.

### "voiceterm has no mail sign-in page"

Correct as a symptom, and it is DELIBERATE. `mail_login_view.rs` exists, is
tested, and is declared at `lib.rs:250` — and has zero callers, which its own
header states and justifies:

> This form has no caller in the egui frontend, and wiring it would be
> theatre. `submit` yields `Credentials` for "the host to hand to the bridge
> (`mail_login`)" — and this frontend has no bridge. The Qt build's
> `mail_login` goes to a `mail_hub`, which is gated behind the `mail-crypto`
> feature and refuses with "this build can't sign in to Proton yet" when it
> is absent.
>
> So a login screen here would collect a password, validate it, and drop it.
> That is worse than no login screen: a form that looks like it works is how
> someone concludes their credentials were wrong.

`mail_store::empty_reason` is what the pane shows instead — it names the
maildir paths tried and says a remote account is a separate thing this build
does not reach.

**So the work is not the form. It is the bridge**, and the blocker is
`TODO_mail_srp_login.md` (Proton SRP). Wiring the form before that exists
would be the regression the header describes.

### "voiceterm has no graph"

Different cause — the graph IS wired. `graph_view` has six references in the
binary and is drawn at `voiceterm_egui.rs:3908`, with focus seeding, depth
controls and breadcrumbs.

What happens on a fresh install is the empty state. `graph_from_notes`
defaults to `true` (line 6311), and with an empty deck the pane draws:

    no notes yet — capture some, or switch to the typed source

That is honest and it reads as absence. The graph of the TYPED source is one
radio button away and is not discoverable from a screen that looks blank.

Worth a small change rather than a port: on an empty deck, either default to
the typed source (which always has something to draw) or make the two-way
selector visible above the message rather than reading as an error. The
selector is already there — `ui.selectable_value(... "my notes" / "typed
source")` — so this is placement, not a feature.

### "rusty-board-egui cardinal has no voice/space/delete/enter row" — CORRECTED

This was written from a `git log -S` search that found no shipped `.toml`
and concluded the layout does not exist. Wrong conclusion, reached by
looking in one place. `Layout::default_cardinal_grid()` in
`crates/board-layouts/src/lib.rs` is a complete, tested, hand-built layout
— MessagEase-style 3x3 with the exact row described: mic, LLM-prompt,
backspace, return, settings. Three sibling variants exist too
(`_dvorak`, `_dvorak_for_anchor`, `_words`).

**It is reachable on both frontends, traced end to end:**

* **GTK**: Settings -> "Keyboard mode" -> Cardinal. A real dropdown
  (`settings_panel.rs:335`), gated on `feature = "input-modes"`, which IS in
  the default feature chain (`default -> build-functioncalling ->
  build-voice -> full-keyboard -> input-modes`). Selecting it sets
  `cfg.layout.experimental_mode = "cardinal"`, which `keyboard_view.rs`'s
  `"cardinal" =>` arm resolves straight to `default_cardinal_grid()`.

* **egui**: `RealBoard::new()` (`real_layout.rs:399`) loads it into the file
  list unconditionally — no feature gate, always present. `demo_app.rs`'s
  sidebar draws a vertical tab per file, labelled from `display_name`
  ("English (Cardinal Grid)" + a keyboard glyph), and clicking one sets
  `self.real.active` and switches. Traced the whole path: no missing
  `pub fn`, no unconstructed type, no untested arm.

**What this means for the report.** Either it describes a build that
predates this wiring (`demo_app.rs`'s sidebar tab strip is recent —
`05c7145b3` touched it two commits before this write-up), or the operator
looked for a settings TOGGLE on the egui build, which has none — the
cardinal grid there is a sidebar tab, a different UI pattern from GTK's
Settings menu, and someone expecting the GTK pattern would report finding
nothing while looking straight at "English (Cardinal Grid) ⌨" without
recognising it as the answer.

The dispatch machinery this section originally credited (`CardinalCell`,
`select_behavior`'s cardinal arms) is not just present, it is EXERCISED —
`default_cardinal_grid()` is constructed in the app's own state
(`demo_app.rs:315`), not only in tests. The original write-up's "every
cardinal path is reachable only from a layout that does not ship" is the
part that does not hold: the layout ships from code, unconditionally, on
both frontends.

**Not corrected: whether the row LOOKS right.** Reading confirms it is
wired and constructs without error; it does not confirm the rendered
gesture directions, key spacing or touch targets match what "the operator
has seen" on the GTK build historically. If the report persists after
finding the "English (Cardinal Grid)" sidebar tab, the gap is almost
certainly a rendering or gesture-mapping difference between the two
frontends' cardinal drawing, not a missing layout — worth checking
`keyboard_ui_styled`'s cardinal-cell rendering against GTK's equivalent
next, with a screenshot from the device rather than another read of the
source.

---

## 15. aperture soft wrap: why it is a layer, not a flag

Asked for alongside two palette fixes (both landed in 10ccbddb5). This one is
scoped rather than started, because it is a change to how the editor
addresses rows and half of it is worse than none of it.

### What assumes one buffer line is one screen row

Every one of these, in `view.rs` unless noted:

| site | assumption |
|---|---|
| `caret.line as f32 * m.line_h` (425, 443, 460, 470) | caret Y is the buffer line number |
| `scroll / m.line_h` (510) | the first visible row is a buffer line index |
| `grid.row_y(line)` (496, 515, 716) | `PageGrid` is indexed by buffer line |
| `click_grid.line_at(p.y)` (402, 411) | a click Y maps to one buffer line |
| `camera::typewriter_target`, `Follow::lines` | both take a caret Y derived the same way |
| `page.rs::line_at` / `row_y` | documented as exact inverses of each other |

`page.rs` is the awkward one: its whole contract is that `line_at` inverts
`row_y`, which is what makes the caret land where the click did. Wrapping
breaks the bijection — one buffer line becomes N rows, and `line_at` can no
longer return a single line.

### The shape it wants

A visual-row layer between the buffer and the grid: `(buffer_line, wrap_ix)`
as the addressing unit, with buffer<->visual maps rebuilt when the width or
the text changes. Then `PageGrid` indexes VISUAL rows and its inverse becomes
`visual_at(y) -> (line, wrap_ix)`, and every site above goes through the map
rather than multiplying.

The right first step is the map itself, pure and tested, in `aperture-core`
next to `page.rs`: given a line's text, a font metric and a width, produce
the wrap points. That is testable without a frame, it is where the
off-by-one bugs live (a wrap point at a space that is then trimmed; a line
whose last word exactly fills the width; a tab), and it can land and be
proven before anything in `view.rs` moves.

### Why not a partial version

Rendering wrapped text without moving the caret math puts the caret in the
wrong place on every wrapped line — the least acceptable regression an editor
has, and one that is not obvious in a screenshot. The order has to be map
first, then addressing, then painting, and the painting is the easy part.

`egui`'s own `Galley` already wraps and reports row/column, so the layout
work is largely "ask epaint and cache it" rather than implementing a line
breaker — worth checking before writing one, since `view.rs` currently calls
`layout_no_wrap` and switching to `layout` returns exactly the rows this
layer needs.

## 16. Two adjacent focus classes, searched and clean

Follow-up to the ten unpinned text fields (0b31df97f). Same method — look for
the CAUSE, not the symptom — applied to the two neighbouring failure modes.
Both came back clean, which is worth recording so the next person does not
re-run them.

### `request_focus()` every frame

28 call sites. Six looked bare; three of those are a test harness
(`ctx.run(RawInput::default())` in pty_input.rs). The two real ones are
palettes — `voiceterm_egui.rs:1490` and `beatspace/arrangepage.rs:862` —
whose comments say "focused on open" while the call fires every frame.

Untidy, NOT broken, and left alone. Focus-stealing only hurts when something
else wants focus, and in both palettes the result rows respond to CLICKS
(which do not need focus) while Enter is read from global input rather than
from the field. `request_focus()` on an already-focused widget is idempotent
in egui. Rewriting them would be churn on working code.

### Duplicate `id_salt` literals

Two files reuse a salt within themselves, and both are correct:

* `uikit-egui/src/layout_probe.rs:227-228` — a test that deliberately
  creates a clash and ASSERTS the probe reports it.
* `aperture/loop_pane.rs:114,155` — two `instruction` fields in different
  arms of one `match state.provider_kind`, so only one exists per frame and
  they can never collide.

### The loop-index variant: checked, three fixed

`id_salt` values built from a loop index rather than a literal ARE
position-derived in the way that matters when a list reorders. 38 `id_salt`
calls take a variable; most key off a real identity (`node.id`, `t.id`, a
label). Five key off a bare index, and reading each was the only way to tell
them apart:

| site | verdict |
|---|---|
| `av1tuner app.rs:1632,1755` | REAL — `Vec::remove` shifts groups; fixed with a stable `MergeGroup::id` (908b710cf) |
| `logscope replicatorui.rs:328` | REAL — same loop already keys by `c.id` four lines up; fixed (b52c27aac) |
| `logscope chat_draw.rs:1144` | REAL — `arts` re-parsed each frame; now keyed by `a.path` (b52c27aac) |
| `bot-arena studio.rs:266` | fine — iterates a static const array |

None of the three is the one-character bug: an index is stable WHILE typing
and only shifts on delete or re-parse, so the symptom is inherited cursor
state or a jumped scroll position, not a dead field. Same root cause, milder
outcome — which is exactly why a symptom-shaped search missed them.

Still open, and worse than any of the above: `chat_draw`'s `self.preview` is
an `Option<usize>` into the same re-parsed `arts` vector, so a renumber makes
it preview a different artifact than the one clicked. That is wrong CONTENT,
not a wrong scroll offset. It is selection state with several call sites, so
it wants its own change rather than being folded into an id sweep.

## 17. studio-video's slate OCR: two real bugs found, one fixed, one is a font limit

Surfaced while verifying the av1-in-rust-studio-live merge (84666efa4), then
investigated properly rather than left as three unexplained failures.

### Bug 1, fixed: '0' and '8' normalised to the identical grid

`normalise()`'s cell-lighting rule was a 1/3 majority vote (`lit * 3 >= n`).
'0' and '8' differ ONLY in the seven-segment middle bar (slot 3), that bar
renders exactly on the boundary between two `CELL` rows at `CELL=8`, and
splits 3 of 12 sampled pixels into EACH of the two rows -- 25% in both,
under the 1/3 threshold. The middle bar vanished from every digit that has
one, so `'0'` and `'8'` normalised to byte-identical `Cell` grids: distance
0, not merely low-confidence. `recognise()` returned `'0'` for the '8'
template read against its own generating set.

Found by a debug test that printed both 8x8 grids and the actual distances
rather than guessing from the source -- `d(8,0)` was 0 where it should have
been small-but-nonzero. Threshold changed to 25% (`lit * 4 >= n`), the
narrowest value that captures the measured 3-of-12 split. A looser `lit > 0`
(any ink at all) was tried first and reverted: it fixed this pair but
fattened every stroke enough that other templates moved closer together
too, dropping the exact-self-match confidence from >0.3 to 0.25 -- trading
one bug for a smaller, harder-to-notice one. Comment in the code records
both the measurement and the reverted attempt.

New test `zero_and_eight_are_distinguishable`, pinned on the templates
directly rather than through `recognise()`, because that is where the bug
actually lived: the two grids have to differ before anything downstream can
be asked to score them.

### Bug 2, NOT fixed: '8' and 'A' are 4 pixels apart, and that is real

With bug 1 fixed, the three tests still fail, but for a different reason:
`recognise(eight_template, &templates())` correctly returns `ch: '8'` now,
with `sure: 0.25` against the test's `> 0.3` floor. The actual runner-up
(found the same way -- printed and measured, not assumed) is `'A'` at
distance 4, and `'8'`/`'A'` differ only in the BOTTOM bar (segment 6) in
this seven-segment rendering. A seven-segment 8 minus its bottom stroke is
a real, generic ambiguity in this font style, not a normalise() defect --
it is what an actual person's handwriting-adjacent seven-segment '8' looks
like when the bottom is short or faint.

Not chased into a second threshold change. Two threshold edits in one
session, the first of which had to be partially reverted for a regression
it introduced, is the point at which the next change wants either a
different distance metric (weighting the outer segments more than the
default per-cell equality) or a redrawn 'A' template -- both real design
decisions about the font, not a bug with an obvious fix. Recorded rather
than guessed at a third time.

`it_does_not_care_which_side_is_ink` and `it_reads_a_board_off_a_frame` fail
for the same underlying reason (both read real boards containing digits
close to this same confidence floor) and should clear once bug 2 does.

---

## 18. voiceprint-kit exists, is unconsumed, and a checker cites a TODO item that was never written down

Found while executing section 13's voice-popup extraction: `phonetic.rs`
and `acoustic_index.rs`, the two modules that made the popup core's real
closure ~4,969 lines instead of ~1,200, ALREADY have a pure extraction —
`shared/voiceprint-kit` (386 lines), whose own header says it was "lifted
VERBATIM from apps/voiceterm (phonetic.rs's MFCC pipeline and
acoustic_index.rs's 39-dimensional utterance fingerprint)."

**voiceterm does not consume it.** `grep -c voiceprint_kit:: src/phonetic.rs
src/acoustic_index.rs` is 0/0. Confirmed the two are still byte-identical
duplicates (`N_MFCC`/`N_MEL`/`FRAME_SIZE`/`FRAME_HOP` match exactly). So
this is the `bo-controller` situation exactly: a correct shared crate
exists, and the original app still carries its own live copy nobody
repointed.

`maintenance/tools/voiceprint_parity_check.py` already guards the drift —
good, and it explains ITSELF clearly why this is not a trivial repoint
("touches voiceterm's AI01->AI02 bundle format, not a small change") — but
its docstring cites the real fix as "recorded as C12-2a in
maintenance/C12_TODO.md," and that file has no such entry. Either the
session that wrote the checker never actually filed the TODO item, or it
was filed and later lost. Either way, the pointer is dead, and a dead
pointer in a checker's own docstring is worse than no pointer: it tells
the next reader information exists where it does not, costing them a
search rather than saving them one — which is exactly what it cost this
session, tracing it down to confirm before writing this entry.

Not fixed here (the repoint is real, non-trivial, bundle-format-touching
work per the checker's own accurate description, not something to attempt
at the tail of an unrelated extraction). Filed HERE instead of inventing a
new C12-2a, since C12's own numbering is not this session's to extend
correctly without the original context that assigned it. Whoever does the
repoint should also fix the checker's docstring to point at wherever the
real record ends up (this section, or a properly filed C12-2a if the
original intent can be reconstructed) — a docstring that cites a phantom
record should not survive past whoever notices it.
