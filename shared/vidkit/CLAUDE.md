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
  `io` holds `ByteSource`: a `read_at`, not a `Read + Seek`, because the two
  real sources are a file and an HTTP server answering range requests, and a
  range request *is* a `read_at`.
- `vidkit-ytdlp` — `yt-dlp -J` parsing, format selection, and the resolve/
  refresh lifecycle. Process invocation is behind `CommandRunner`. `http` is a
  `ByteSource` over range requests, with the socket behind `HttpTransport` so
  the cache and the 403 path are tested with no network — the real transport
  is a thin wrapper this crate deliberately does not depend on.
- `vidkit-mp4` — ISOBMFF demuxer.
- `vidkit-webm` — Matroska/EBML demuxer.
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

**Exists but is not wired:** `HttpByteSource`. Nothing joins `Resolver` to it
to a demuxer yet, so yt-dlp output still cannot reach playback. The pieces are
each tested and untested TOGETHER, which is the gap `tests/pipeline.rs` exists
for.

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
cargo test --ignore-rust-version            # 322 tests
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p vidkit-vk
python3 tools/mutate.py                     # 126 mutants, all must be killed
python3 tools/mutate.py shader              # just the WGSL ones
```

`tools/mutate.py` injects one realistic bug at a time and reports which the
suite fails to notice. It sets `VK_ICD_FILENAMES` to lavapipe itself when
nothing else is configured, because the Vulkan tests skip without a device and
a skipped test kills no mutant, and it puts a packaged rustc 1.91 on PATH when
one exists.

**Run it detached.** A full sweep outlives a five-minute harness call, and a
killed run used to leave the source mutated. It now records the original in
`.mutate-inflight` before editing and replays it on the next start, but
`nohup setsid python3 tools/mutate.py > /tmp/sweep.log 2>&1 &` and polling the
log is still the way to run it.

**A full disk is not a test failure.** It used to report as `baseline FAILING`
and, worse, as mutants *killed* — cargo failing to compile looks exactly like a
test catching something, and one gesture sweep reported 16/16 while sitting
against the disk wall when two of those genuinely survived. The runner now
raises on `os error 28` and stops. If a sweep reports a suspiciously perfect
score, check `df -h /` before believing it.

Deliberate survivors live in `EXPECTED_SURVIVORS` with a written reason.
Anything else that survives is untested behaviour: write a test or delete the
code.

## Why mutation testing is not optional here

Nine tests in this repo have passed for the wrong reason at some point. The
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
- `iter().all(...)` is true of an empty iterator. Assert the collection is
  non-empty *first*, then assert what is in it. A gesture test passed against a
  handler that emitted nothing at all.
- A symmetric round trip does not test quantisation: twenty steps up and twenty
  down returns to the start under naive float addition too. Assert the
  invariant at every intermediate value, not at the endpoints.
- Assert on the type the call sites use. A percentage test written against f64
  literals could not fail, because no multiple of 0.05 differs between rounding
  and truncation in f64. The real values are f32, where `1.05f32 as f64`
  truncates to 104.

Run the mutation sweep before trusting a refactor.

## The seams are tested, not just the layers

`crates/vidkit-egui/tests/pipeline.rs` runs the real `Mp4Demuxer` through the
real `Player` into the real `EguiPresenter`, with only the codec stubbed —
nothing decodes in this sandbox, and the codec is not what the seams are about.
Its MP4 comes from `vidkit_mp4::build`, public behind a default-off `fixtures`
feature, so there is one fixture writer rather than two free to drift.

Both demuxers are covered, and WebM is the one that matters most: it is the
Fairphone's path (YouTube VP9 is WebM, and VP9 is what Venus decodes in
hardware), and it answers the player with entirely different mechanisms —
nanosecond timestamps scaled by `TimestampScale` rather than timescale ticks, a
block timestamp that is its *cluster's* plus a signed offset, keyframes from a
flag byte rather than an `stss` table, and no Annex-B rewrite at all.

Every layer here was green in isolation long before this existed. Breaking
three MP4 seams found the test caught only one of them on the first attempt,
and both misses were fixtures in which the constraint could not bind — seeking
to a time that happened to be a keyframe, and seeking after the decoder queue
had already drained. The WebM round then caught four of four, because those
fixtures were written already knowing the shape. The `seam/` mutants keep it
honest.

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

## The egui surface

`vidkit-egui` is the front end, on `shared/uikit`. `EguiPresenter` converts on
the CPU and uploads an egui texture — portable to the glow backend everything
in this tree uses; the zero-copy DMA-BUF path is `vidkit-vk` and needs wgpu.
`VideoView` paints it letterboxed with the kit's carved skin, and the bottom
bar adapts through `uikit_egui::adaptive`.

Gestures are routed by a pure state machine (`gestures.rs`): pointer positions
in, `Intent`s out, no playback state, so the rules are tested without a frame.

```
horizontal drag anywhere    scrub, rate set by pointer HEIGHT (higher = faster)
left half, vertical         in-app volume, 5% detents
right half, vertical        playback speed, 5% detents
right EDGE strip, vertical  rotate 90 degrees
pinch                       5% zoom detents; far enough snaps to fit/fill
tap                         play/pause
```

Two rules stop them fighting, both tested: the **zone is decided at press**,
not continuously, so a drag that wanders across the midline does not change
what it controls; and the **axis locks once**, after 8px, with nothing emitted
before it.

Two readings of the original request are encoded as single knobs, in case
either was wrong. `rotate_edge_fraction = 0.0` removes the rotate strip if
"right side" meant the same thing as "right half". Swapping
`scrub_span_at_bottom` and `..._at_top` inverts the scrub ramp to match mpv and
VLC, which give *finer* control further from the bar.

**Look at it, do not just compile it.** `docs/agents/HEADLESS_GUI_TESTING.md`
applies:

```sh
cargo run -p vidkit-egui --features demo --example video_demo --ignore-rust-version
```

Screenshots caught three things no test did: the bar painting over itself,
rotation panicking on first use (`Mesh::colored_vertex` asserts the mesh has no
texture, and painting needs a live context so nothing headless reached it), and
the compact layout needing to exist at all. Resize the running window with
`xdotool windowsize` and check BOTH breakpoints — a layout that is always
compact looks correct in a single screenshot.

## Not built

Fragmented MP4 (`moof`), edit lists, audio track extraction from either
container, Cues/`sidx`-driven seeking (both demuxers index the whole file at
open, which over HTTP means reading all of it), the dav1d and VAAPI decoder
backends, the audio output backend, and the UI shell.
