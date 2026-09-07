# CLAUDE.md

Working notes for this repo. Read this before changing anything; it records the
decisions that are load-bearing and the ones that are still guesses.

## What this is

A video player core targeting Asahi Linux and Fairphone/postmarketOS, fed by
yt-dlp or local files. Headless: no UI in this tree. The UI shell plugs in at
the `Presenter` seam in `vidkit-core::traits`.

## The one fact the whole design rests on

**The two targets have opposite codec preferences.**

| | H.264 | HEVC | VP9 | AV1 |
|---|---|---|---|---|
| Asahi (M1/M2) | AVD fixed-function (V4L2 stateless) | software | **software** | **software** |
| Fairphone (Venus) | fixed-function | fixed-function | **fixed-function** | **software** |
| Desktop AMD/Intel | VAAPI | VAAPI | VAAPI | VAAPI |

So on Asahi prefer AV1 (dav1d beats ffvp9); on the Fairphone prefer VP9 (Venus
hardware beats dav1d by an order of magnitude in power). Hardcoding either is
wrong on one device. That is why `caps.rs` is a runtime data model rather than
`#[cfg]` branches, and why `select.rs` scores against it.

Status of that table: the Asahi row is **unverified**. It comes from the AVD
V4L2 driver supporting H.264 only and Honeykrisp not exposing
`VK_KHR_video_decode_*`. An attempt to confirm the latter by grepping Mesa's
driver `.so` files was inconclusive — every driver contains the extension name
strings, including lavapipe, because the table lives in shared runtime code.
Do not treat that grep as evidence.

## Crates

- `vidkit-core` — types, traits, capability model, sync policy, frame queue,
  clocks, player loop. No GPU, no toolkit, no process spawning. All pure.
- `vidkit-ytdlp` — `yt-dlp -J` parsing, format selection, and the resolve/
  refresh lifecycle. Process invocation is behind `CommandRunner`.
- `vidkit-mp4` — ISOBMFF demuxer. Also owns `ByteSource` and the shared IO
  error type.
- `vidkit-webm` — Matroska/EBML demuxer. Depends on `vidkit-mp4` for
  `ByteSource` so the two cannot disagree about what a truncated file is.
- `vidkit-v4l2` — stateful decoder state machine, uAPI structs, device layer.
- `vidkit-vk` — convert shader plus a headless harness that runs it; DMA-BUF
  import path.

## Verification status — read before claiming anything works

**Executed and asserted:** codec parsing, capability model, format selection,
yt-dlp resolve/refresh, sync policy, frame queue, player loop, audio clock,
V4L2 state machine, V4L2 struct layouts and ioctl numbers (checked against
`/usr/include/linux/videodev2.h` via a C program), MP4 and WebM demuxing, and
the convert shader's colour maths (run on lavapipe).

**Compiles, never run:** `vidkit-v4l2::device` — no V4L2 device has ever been
opened, not even `vicodec`. The struct layouts underneath it are verified,
which removes the largest failure class, but the mmap/EXPBUF/QBUF sequencing
against a live driver is untested. Expect first-run failures.

**Unverified entirely:** the DMA-BUF import path in `vidkit-vk::import`.
Lavapipe does not expose `VK_EXT_image_drm_format_modifier`, so the harness
uploads planes through a staging buffer instead. The modifier, disjoint-plane,
memory-type and queue-ownership rules in that module's docs are reasoned, not
tested.

**Estimates, not measurements:** every `max_fps_at_max_size` and max resolution
in the reference `DeviceCaps` profiles. Measure on real hardware before
trusting them.

## Commands

```sh
cargo test                                  # 206 tests
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p vidkit-vk
python3 tools/mutate.py                     # 89 mutants, all must be killed
python3 tools/mutate.py shader              # just the WGSL ones
```

`tools/mutate.py` injects one realistic bug at a time and reports which the
suite fails to notice. It sets `VK_ICD_FILENAMES` to lavapipe itself when
nothing else is configured, because the Vulkan tests skip without a device and
a skipped test kills no mutant.

Deliberate survivors live in `EXPECTED_SURVIVORS` with a written reason.
Anything else that survives is untested behaviour: write a test or delete the
code.

## Why mutation testing is not optional here

Six tests in this repo have passed for the wrong reason at some point. The
recurring shapes:

- Asserting only `is_err()`. The broken code usually still errors, just later
  and with a misleading message. Assert the variant or the message.
- Reading the expected value back from the component under test. A clock that
  reports the wrong position also moves the yardstick. Compute ground truth in
  the test.
- Fixtures where the constraint cannot bind — more video than audio, so a
  "did not run ahead" bound is never exercised.
- Recycle-token leaks are invisible on software decode (`recycle: None`) and
  only appear as a freeze minutes into hardware playback. Tests count buffers.

Run the mutation sweep before trusting a refactor.

## Invariants that are easy to break

- **Every recycle token gets back to the decoder, on every path.** Frames leave
  the pool through four exits: presented, dropped late, dropped under pressure,
  discarded on flush. `FrameQueue::take_reclaimed` must be drained.
- **`RecycleToken` is not `Copy`** and carries a generation. A token minted
  before a resolution change must not requeue a buffer that now belongs to
  something else.
- **The V4L2 CAPTURE queue is untouched until the first `SOURCE_CHANGE`**, and
  a mid-stream resolution change leaves the OUTPUT queue streaming.
- **`min_capture_buffers` + headroom.** The driver's minimum covers reference
  frames only; allocate exactly that and playback stalls one frame in.
- **H.264/HEVC from MP4 must be converted to Annex-B** with parameter sets
  re-emitted after every seek. A V4L2 decoder fed MP4-format samples outputs
  nothing and reports no error.
- **Work per player tick is bounded.** Unbounded draining blocks the UI thread
  after a seek.

## Platform gotchas

- `libc::ioctl`'s request parameter is `c_ulong` on glibc and `c_int` on musl.
  postmarketOS is Alpine, so the phone is musl. Cast with `as _`.
- `v4l2_buffer` embeds `struct timeval`; its size is ABI-dependent, not just
  arch-dependent. 88 bytes verified on 64-bit. Re-extract for armv7.
- `v4l2_pix_format_mplane` is `__attribute__((packed))` — needs
  `repr(C, packed)`.
- Never assume `DRM_FORMAT_MOD_LINEAR`; tiled buffers give a black or smeared
  frame with no validation error.
- `wgpu`'s `texture_from_raw` and `as_hal` signatures churn between releases
  and will not fail to compile gracefully. Pin wgpu.
- yt-dlp URLs are IP- and header-bound and expire. Never hand-construct or edit
  one: YouTube's `n`-parameter throttling drops throughput to ~50 KB/s and
  presents as buffering, not an error.

## Where this sits in the monorepo

`shared/vidkit`, a nested workspace, which is the house pattern — there is no
root cargo workspace in this tree (`LAYOUT.md`: "nested workspaces + C++/cmake
can't be one"), and `shared/uikit` is laid out the same way. Nothing had to be
excluded or flattened.

Build it the house way, one crate at a time:

```sh
CARGO_TARGET_DIR=/tmp/bd CARGO_BUILD_JOBS=1 \
  maintenance/tools/cargo-ailite check
```

**Toolchain.** This tree wants rustc >= 1.91; `shared/uikit`'s `Cargo.lock` is
lockfile v4, which cargo 1.75 cannot even parse. Ubuntu packages 1.91
(`apt-get install rustc-1.91 cargo-1.91`, then prepend
`/usr/lib/rust-1.91/bin` to `PATH`) which is enough for everything here
including `uikit-egui` — egui 0.34 declares MSRV 1.92 but builds clean on 1.91
with `--ignore-rust-version`, as `manifest/projects.toml` already records for
aperture. Verified, not assumed.

`Cargo.lock` is committed, as everywhere in this tree — see
`maintenance/tools/lock_check.py` for why a stale one is worse than none.

## Not built

Fragmented MP4 (`moof`), edit lists, audio track extraction from either
container, Cues/`sidx`-driven seeking (both demuxers index the whole file at
open, which over HTTP means reading all of it), the dav1d and VAAPI decoder
backends, the audio output backend, and the UI shell.
