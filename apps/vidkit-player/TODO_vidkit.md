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
| transcription | `Transcriber` (voiced-core) | app | **not written** |
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

1. **`Transcriber` impl (whisper-rs).** Unblocks subtitles, scrub-to-sections
   and the voice router in one go — three palette rows currently say "not wired
   up yet". `voice-kit` already does the post-processing and
   `voiced-core::grammar` already does the matching; only the mic and the model
   are missing. *read*
4. **Tooltip fade-in.** Asked for and done — see below — but only the shared
   helper; individual controls still need `.on_hover_text` replaced. *verified*
5. **`Inhibitor` impl.** wlr idle-inhibit on Phosh, `org.freedesktop.ScreenSaver`
   elsewhere. The policy is written and tested. *read*
6. **480p + optical flow interpolation.** Never started. `apps/bscale`'s
   `d2x-core` has block **matchers**, and block matching *is* motion estimation
   — if they expose per-block displacement rather than burying it in the
   residual path, most of an interpolation front end already exists with a CPU
   reference to check against. **I never opened that code.** *assumed* — verify
   before planning around it.
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
