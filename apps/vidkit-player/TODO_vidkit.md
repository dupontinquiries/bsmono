# TODO — vidkit (player + kit)

Handover for whoever picks this up. Every line is marked with how it was
established, per the root `TODO.md` convention: **verified** (I ran it and saw
the result), **read** (I read the code), **assumed** (neither — treat as a
guess).

Two pieces:

- `shared/vidkit` — the kit. Nested workspace, 472 tests, 181 mutants.
- `apps/vidkit-player` — the app. Registered in `manifest/projects.toml` as
  `role = "app"` and offered in gitmind's fleet store.

```sh
# 1.91 or newer; shared/uikit's lockfile is v4 and cargo 1.75 cannot parse it.
export PATH=/usr/lib/rust-1.91/bin:$PATH
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json   # or shader tests SKIP
cd shared/vidkit && cargo test --ignore-rust-version
nohup setsid python3 tools/mutate.py > /tmp/sweep.log 2>&1 &   # outlives a 5-min call
```

## Where the seams are

Everything expensive or unavailable sits behind a trait, so the policy above it
is testable with no device, no network and no model. This is the pattern to
keep:

| seam | trait | real impl lives | status |
|---|---|---|---|
| yt-dlp process | `CommandRunner` | `src/io.rs` | ✅ written, wired, tested with a scripted extractor — *verified*; never run against real yt-dlp — *assumed* |
| HTTP range fetch | `HttpTransport` | `src/io.rs` | ✅ plain TCP + `ureq`/rustls, picked by scheme — *verified* for both, TLS against a real HTTPS origin |
| V4L2 decode | `V4l2Api` | `vidkit-v4l2::device` | compiles, never run — *verified* it builds, *assumed* it works |
| transcription | `CueSource` (vidkit-transcribe) | `vidkit-transcribe::whisper`, `::subtitles`; loop in `src/transcribe.rs` | sidecar VTT/SRT *verified* end to end incl. screenshots; whisper-rs 0.16 *compiles*, **never run** (no model obtainable in the sandbox) |
| audio output | `AudioSink` | `src/audio_out.rs` (cpal, feature `alsa`) | ring + engine clock *verified* against a simulated device; cpal *verified* to open/drain/pause on ALSA `null`; **never run on a real card** |
| relevance model | `Analyzer::take_pending`/`fulfill` | app | **not written** |
| screen inhibit | `Inhibitor` | app | **not written** |

None of those five "not written" pieces is hard; each is a thin wrapper with no
logic. They are unwritten because standing one up needs a device, a network, a
model or a compositor that could not be supplied or tested here, and writing an
untestable wrapper would have looked like progress.

## Done and verified

- Container demuxing, MP4 and WebM, incl. Annex-B conversion — *verified*
- Format selection against `DeviceCaps`. Three profiles with genuinely opposed
  preferences, each pinned by a test: Fairphone/QCM6490 Venus (VP9 fixed-
  function, no AV1), Asahi M1/M2 (neither in hardware; AV1 fastest in software
  via dav1d), Asahi M3 (**AV1 fixed-function**, and no usable GPU). *verified*
  by test, *assumed* on real hardware — the M3 resolution/fps limits in
  particular are placeholders, since the announcement says AV1 works and not to
  what limits
- `vidkit_v4l2::sysprobe` reads the device-tree SoC id, the V4L2 driver names
  and the bound DRM driver, so the profile is PROBED rather than guessed from
  the target triple — which could not tell an M1 from an M3 from a Fairphone.
  *verified* against fixture filesystem roots; never run on real hardware, so
  the paths it reads are *read* from documentation
- `GpuClass`: hardware decode does NOT imply a usable GPU. M3 decodes AV1 into
  DMA-BUFs with no driver to import them, so every frame is mapped and
  converted on the CPU — `frames_need_cpu_copy` reports it. *verified*
- 10-bit (P010 + I420_10LE) — *verified*, incl. GPU/CPU parity on lavapipe
- Player loop, A/V sync, frame queue, buffer recycling — *verified*
- Gestures: scrub / volume / speed / rotate / pinch — *verified* by xdotool
- Auto-hiding chrome, settings popup, palette — *verified* by screenshot
- NV12 GPU upload path — *verified* pixel-identical to the CPU path **on
  llvmpipe**; *assumed* on turnip and AGX. Opt-in via `VIDKIT_GPU=1`
- Selective streaming: relevance plan → byte ranges → clipped read-ahead —
  *verified*
- The full page path against REAL yt-dlp 2026.08.19: resolve, select, fetch,
  sniff, demux — *verified* by `the_real_extractor_is_parsed_and_played`, which
  skips cleanly when yt-dlp is absent

## Open, in the order I would do them

0. **Fragmented MP4 (`moof`/`trun`).** *verified* against a real yt-dlp `f616`
   download: ftyp brand `dash`, a 688-byte `moov` with an EMPTY sample table,
   then moof/mdat pairs. This is what YouTube serves and what yt-dlp fetches
   fragment by fragment, so it is the ORDINARY case here, not an exotic one —
   and it is the cause of the "0 packets" reports. `vidkit-mp4` reads sample
   tables from `moov` only; every sample in a DASH file lives in `trun`.
   Playback is unaffected because ffmpeg demuxes, so this shows up as a wrong
   description rather than a broken video. The demuxer now says which thing is
   unsupported instead of reporting a corrupt `stsc`.


1. **Run whisper on a device.** Put `ggml-base.en.bin` (or tiny on a phone,
   `WHISPER_PREFER=speed`) in `~/models/whisper`, open a lecture with no
   sidecar, open Sections. Watch the realtime factor in the panel's status
   line: above 1.0 the transcript falls behind playback, and `Ahead` scope is
   the right default on that device. *assumed* until someone does this.
2. **Hardware decode on real hardware.** `decode::choose_accel` picks
   v4l2m2m on Venus and VAAPI on i915/xe/amdgpu, with a software fallback if
   no first frame arrives. The status line names what is decoding. Never run
   here -- no /dev/dri, no V4L2 decoder. *read*
3. **The cpal sink on a real card**, and compare `VIDKIT_AUDIO=pipe`: seek
   gaps and A/V offset are the two things to listen for. *assumed*
4. **Tooltip fade-in.** Asked for and done — see below — but only the shared
   helper; individual controls still need `.on_hover_text` replaced. *verified*
5. **`Inhibitor` impl.** wlr idle-inhibit on Phosh, `org.freedesktop.ScreenSaver`
   elsewhere. The policy is written and tested. *read*
6. **480p + interpolation. The motion search exists; the interpolation does
   not.** *verified* — I finally read `apps/bscale/crates/d2x-core/src/`
   instead of guessing, having marked this "assumed" five times.

   What is there and is directly reusable:

   - `matcher::BlockMatcher` — `search(prev, next, dst_x, dst_y, size, limit)
     -> Option<Candidate { src_x, src_y, sse }>`. The motion vector is
     `(dst_x - src_x, dst_y - src_y)` and `sse` is a usable confidence. Three
     implementations: `Diamond` (fast), `Exhaustive` (reference), `Stationary`
     (zero-motion baseline).
   - `search_from(.., hint)` takes a motion hint and falls back to the unhinted
     search — predictive/hierarchical estimation for free, and the doc already
     notes why the fallback matters: "a moving actor does not follow the
     camera".
   - `phase_corr::PhaseCorrelation` — global sub-pixel translation with a
     confidence where 0 means "scene cut or content too uniform", which is
     exactly the signal an interpolator needs to stop interpolating.
     Convention pinned in its docs: `estimate(cur, prev)` gives
     `cur[x] ≈ prev[x - dx]`.

   What is NOT there, and is most of the work:

   - Bidirectional vectors and occlusion handling. Block matching gives
     backward vectors only; a synthesised intermediate frame needs both
     directions and something to do about holes.
   - Sub-pixel per-block motion. `Candidate` is integer; only the GLOBAL
     estimate is sub-pixel, and 24→90 needs fractional positions.
   - Any motion-compensated blend at all.
   - These take `d2x_core::Frame` (RGB); vidkit has NV12 planes.

   **Layering blocker.** `d2x-core` is an app crate. `shared/vidkit` depending
   on `apps/bscale` inverts the layering. `phase_corr.rs`'s own header already
   says the house pattern is to lift it into a `shared/` kit and points both
   consumers at it, and says that lift has not been done — vidkit would be the
   third consumer, which is the argument for doing it. **That lift is the
   prerequisite, and it is a separate mechanical commit touching fao-codec and
   bscale.**

   No measurement exists for whether 480p block matching sustains the frame
   budget for 24→90 on a phone. Do not plan around it until someone times it.

7. **Migrate the app palette's widget onto `uikit_egui::command_bar`** more
   fully, and reconsider whether `vidkit-egui::palette` should exist at all now
   that it is only an action set. *read*

## The glass panel

Translucency and readability are *verified* — `glass.rs` picks the tint alpha
from the WCAG contrast arithmetic so text clears 4.5:1 over a white frame AND a
black one, and `backdrop_visible` fails if a tweak makes the panel opaque
enough that the video behind it disappears.

The softening is *verified* too, and honestly described: the panel is one
`Area` compositing backdrop and tint into a single rect, and six offset copies
at a 14-point radius turn hard band edges into gradients. It is ghosting, not
a convolution. A real separable blur needs the glow path and a render target,
which would make the settings panel depend on the GPU feature — worth doing
deliberately, not by accident. *verified* by screenshot at each step; two
earlier attempts (blur behind a Window, and a 3-point radius) did nothing
visible and the screenshots are what said so.

## Traps, all paid for once already

- **The app already had a palette and a file picker.** I added a second palette
  before reading `apps/vidkit-player/src/`, and it reimplemented
  `uikit_egui::fuzzy::score`. Read the app before adding to the kit. *verified*
- **Registering in `manifest/projects.toml` is not enough** to appear in
  gitmind — `cockpit::flow::store_groups()` is a separate hand-maintained list.
  `every_registered_app_is_offered_in_the_store` now catches it. *verified*
- **A skipped Vulkan test kills no mutant.** Without the ICD set the shader
  tests skip and the sweep reports a perfect score. *verified*
- **A full disk presents as a test failure**, and once as a truncated
  `git bundle`. `tools/mutate.py` now detects it. *verified*
- **`tadpole` is `role = "app"` with no `[[bin]]`** — pre-existing, recorded in
  `KNOWN_MISSES`, not mine to fix. *verified*

## The recurring failure, worth internalising

Roughly a dozen tests in this tree passed for the wrong reason before being
fixed, and nearly all were one shape: **a fixture in which the assertion could
not bind.** More video than audio, so a "did not run ahead" bound never
engaged. A square surface hiding a pixel-aspect difference. A seek target that
was already a keyframe. A drained decoder queue with nothing stale to leak. A
2×2 buffer that reallocates to the same capacity it reuses.

`tools/mutate.py` exists because of this. A green suite is not evidence; a
suite that fails when you break the thing it names is.
