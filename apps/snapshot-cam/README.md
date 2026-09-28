# snapshot-cam

A local camera: take a photo, keep it on this device, and hand it to
photosync or a snapclone contact when you choose to. Tracks disclosure
state explicitly — a capture starts local, and sharing/sending is a
distinct, trackable transition, not implied by the photo simply existing.

## Verification (2026-08-19)

Had no build-status record anywhere. A session with a real rustc 1.96
(clears egui 0.34's MSRV, and this crate's default `app` feature pulls the
full `wgpu` eframe backend, not just `glow`) ran it for the first time:
`cargo test` is 14/14 — capture-is-born-local, disclosure tracking (sending
counts as disclosure, backing up does not), re-targeting a failed send,
refusing a second concurrent send, filename generation defends against a
device name breaking the path, and the roll answers "what have I shared"
newest-first. Compiles clean; one pre-existing `egui::CentralPanel::show`
deprecation warning (0.34 renamed it `show_inside()`), not touched.

## Now a camera (2026-09-27)

The capture stub is gone. `camera.rs` probes the machine (pw-dump, v4l2-ctl)
and runs `shared/capture-kit` — snapclone's capture engine, hoisted into a kit
so this app does not carry a second copy — as a gst-launch subprocess reading
RGBA frames; the viewfinder draws them (mirrored for the front camera; the
file is never mirrored). The shutter writes a JPEG under
`$XDG_DATA_HOME/snapshot-cam/roll/` and the roll persists every disposition
in `roll.tsv` beside it. Both share targets have a transport:

- **back up** copies into photosync's library folder (read from photosync's
  own settings), atomically, and photosync's next scan shows it;
- **send to <contact>** drops a request in snapclone's outbox
  (`shared/snap-handoff`); snapclone sends it through its ordinary snap path
  and writes a marker back, which the roll reads as "shared with Bee".

Contacts come from snapclone's `contacts.conf`. `SNAPSHOT_CAM_TEST_PATTERN=1`
runs the kit's synthetic camera. Verified headless: 42 tests; the full loop
(capture → outbox → snapclone A → snapclone B's inbox → marker → roll) walked
with two snapclone instances on one host. NOT RUN: a real camera —
`gst-launch-1.0` was not on the authoring machine.

### Digital zoom (2026-09-27)

Pinch on the viewfinder (two fingers on a screen, a trackpad pinch, or
ctrl+wheel with a mouse); `+`/`−` step and `0` resets when no text field has
the keyboard; a one-finger drag pans while zoomed; the chip on the picture
says the magnification and a tap on it goes back to 1×. The model is
`shared/uikit`'s `zoom::Zoom` — a window into the frame in frame fractions,
pinched about the point between the fingers — and the same window is what
the shutter saves: the photo is the window at the sensor's native pixels
(`RawFrame::crop`), never upscaled, so a 2× photo of a 1280×720 frame is a
640×360 file. Verified headless with the test pattern (the shell test) and
with ctrl+wheel under Xvfb through real `gst-launch-1.0` (shots sc-15/16:
2.2× → a 576×323 file). NOT RUN: a real two-finger pinch — no touchscreen
in the sandbox; the two-finger path is the kit's touch bridge, tested with
synthetic contacts in `shared/touchkit`.

### DSLR controls and lens profiles (2026-09-27)

The **pro ▸** toggle under the viewfinder opens:

- **lens**: a profile per clip-on lens. Built-ins for Moment's line carry
  only what the lens design fixes (anamorphic 1.33×/1.55× desqueeze;
  autocrop on for the wide, fisheye and tele; peaking and a working-distance
  note for the macro). Distortion (Brown k1, k2), vignetting (lensfun `pa`)
  and focus are zero or "camera" until you calibrate them, because
  invented numbers would only move the error. Profiles live in
  `$XDG_CONFIG_HOME/snapshot-cam/lenses/*.conf`; a file with a built-in's
  name replaces it, and files that fail to parse are reported by name.
- **autocrop**: finds the lens's black border (dark region connected to the
  frame edge), crops to the largest frame-shaped rectangle inside the lit
  area, re-measures every 400 ms with a deadband, **lock** pins it,
  **save** writes the profile (with the current camera focus).
- **focus per lens**: `focus = camera | auto | 0–1` (a fraction of the
  camera's focus range) is applied through the camera's control when the
  lens is picked.
- **EV** ±3 in thirds; **WB** as shot, auto (grey world, inside the crop),
  presets or kelvin.
- **aids**: RGB histogram, zebras (≥250), focus peaking, thirds grid, and a
  readout with a meter (stops against 18% grey) and the clipped fraction.
- **camera controls** (V4L2 cameras only, through `v4l2-ctl`): shutter
  time, gain, WB temperature, focus, and their automatics.

One develop pass (`develop.rs`) does lens correction, crop, zoom, desqueeze,
WB and EV in linear light. The preview runs it on a worker thread at preview
size (latest frame wins); the shutter runs the same function at native
pixels. Honest limits: the frames are 8-bit post-ISP, not RAW, so EV cannot
recover a clipped highlight. Real exposure needs the camera controls, and
those were not run against a real V4L2 device here (parse and command are
tested against captured output and a stand-in script). PipeWire/libcamera
cameras expose no V4L2 controls, and the panel says so.

### Triggers, multi-shot merges, cover correction, layout (2026-09-27)

- **Triggers** (pro ▸ trigger): the volume keys, the power key (opt-in;
  a `systemd-inhibit --what=handle-power-key` child keeps logind off it
  while on) and a shake (IIO accelerometer, several reversing jolts within
  0.8 s). A trigger PRIMES the shutter: every frame in the next second is
  scored for sharpness and the sharpest is the photo, so the jolt of the
  press is not what gets saved. Keys come from `/dev/input/event*` (found
  via `/proc/bus/input/devices` by the keys each device declares). A small
  button device (a phone's volume rocker) is grabbed so a press does not
  also change the volume; a keyboard never is.
- **Shots**: single; **HDR** (exposure bracket −2/0/+2 stops through the
  camera's exposure control, merged by exposure fusion — Mertens, pyramid
  blend); **focus stack** (5 positions across the focus range, merged by
  local sharpness); **burst merge** (6 frames averaged in linear light,
  for noise); **WB bracket** (one frame, three files: as for 4500K, the
  setting, 6500K). Frames are aligned (translation, coarse to fine) before
  merging; the sequence waits 4 frames after each control change and puts
  every control back when it ends. HDR and focus need V4L2 controls; the
  panel says so where there are none.
- **Cover**: the plastic/glass over the camera. "calibrate" measures it
  from one frame — a grey card in the middle (tint → per-channel gains)
  and something black in the frame (haze → a per-channel flare offset) —
  saves `cover.conf`, and the develop pass undoes both.
- **View**: full screen (the viewfinder is the window, the roll a page),
  compact (picture with the roll under it), or auto (full on a phone-sized
  window).

Timing (x86, release, 1280×720 ×3): fusion 525 ms, focus stack 370 ms,
average 108 ms, on a worker thread. A 6-frame burst through real gst was
~5 s end to end in a debug build.

### Running it

- `snapshot-cam` — probes `pw-dump` and `v4l2-ctl`, opens the first camera
  through `gst-launch-1.0` (which must be on PATH at runtime).
- `SNAPSHOT_CAM_SOURCE=/dev/video2`, a PipeWire node name, or
  `gst:videotestsrc pattern=ball` — override the source. The `gst:` form runs
  the real GStreamer path on a machine with no camera.
- `SNAPSHOT_CAM_TEST_PATTERN=1` — the in-process synthetic camera; no
  GStreamer needed.
- Photos: `$XDG_DATA_HOME/snapshot-cam/roll/` (deleted ones go to `.trash/`
  beside them); the roll and its dispositions in `roll.tsv` there; facing and
  fill in `$XDG_CONFIG_HOME/snapshot-cam/settings.conf`.
- Sends: `$XDG_DATA_HOME/snapclone/outbox/<id>.request`, answered by a running
  snapclone with `<id>.sent` or `<id>.failed` (see `shared/snap-handoff`).
  A photo backed up after being sent still says who has it — the record is
  sticky, on purpose.
