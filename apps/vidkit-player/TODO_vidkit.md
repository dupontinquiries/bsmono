# TODO — vidkit (player + kit)

Handover for whoever picks this up. Every line is marked with how it was
established, per the root `TODO.md` convention: **verified** (I ran it and saw
the result), **read** (I read the code), **assumed** (neither — treat as a
guess).

Two pieces:

- `shared/vidkit` — the kit. Nested workspace, 462 tests, 177 mutants.
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
| yt-dlp process | `CommandRunner` | app | **not written** |
| HTTP range fetch | `HttpTransport` | app | **not written** |
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
- Format selection against runtime `DeviceCaps`; Asahi prefers AV1, Fairphone
  prefers VP9 — *verified* by test, *assumed* on real hardware
- 10-bit (P010 + I420_10LE) — *verified*, incl. GPU/CPU parity on lavapipe
- Player loop, A/V sync, frame queue, buffer recycling — *verified*
- Gestures: scrub / volume / speed / rotate / pinch — *verified* by xdotool
- Auto-hiding chrome, settings popup, palette — *verified* by screenshot
- NV12 GPU upload path — *verified* pixel-identical to the CPU path **on
  llvmpipe**; *assumed* on turnip and AGX. Opt-in via `VIDKIT_GPU=1`
- Selective streaming: relevance plan → byte ranges → clipped read-ahead —
  *verified*

## Open, in the order I would do them

1. **`Transcriber` impl (whisper-rs).** Unblocks subtitles, scrub-to-sections
   and the voice router in one go — three palette rows currently say "not wired
   up yet". `voice-kit` already does the post-processing and
   `voiced-core::grammar` already does the matching; only the mic and the model
   are missing. *read*
2. **`HttpTransport` impl.** `StreamSource` and the section plan are both
   finished and have no way to reach the network. A `ureq` wrapper with no logic
   in it. *read*
3. **`CommandRunner` impl.** Same shape; `std::process::Command` around
   `yt-dlp`. *read*
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
