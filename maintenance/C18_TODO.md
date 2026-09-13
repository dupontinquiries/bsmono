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
