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
