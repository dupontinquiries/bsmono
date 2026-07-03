# Camera diagnostics: `sc-camdiag` + unitized gate logic

This is a hardware-validation harness so we can debug camera/preview issues
iteratively instead of guessing from logs. Two parts:

1. **`sc-camdiag`** — a tiny standalone binary you run ON the Fairphone 5. It runs
   the *exact* gst-launch pipeline the app uses, reads the raw frames, and prints
   the REAL numbers (true frame count, measured fps, inter-frame stalls, stride
   validity, freeze heuristic). It does NOT touch the app or its DB.
2. **Unitized gate logic** in `snapclone-videosrc` — the per-frame
   process/pause decision (`frame_gate`) and a debounce variant
   (`frame_gate_debounced`) are now pure, tested functions, so the behavior the
   device log hints at ("PAUSED visible:true appActive:false") is verifiable in
   `cargo test` and replayable.

## Why this exists (what the last log showed)

- The app's `frames-seen: N` was actually the size of a 30-entry fps ring buffer,
  so it always capped at 30 — it told us nothing about real delivery. The app now
  logs the **real** per-session count (`frames-seen: N (fps-window: M)`).
- The preview pauses whenever the app loses ACTIVE state, even while the camera
  page is fully visible (`PAUSED visible:true appActive:false`). On desktop/Plasma
  that can flip for transient reasons (notifications, focus). `sc-camdiag` lets us
  confirm whether the *pipeline* is healthy independent of the app's gating.

## Build (on the device, or cross-compiled)

```sh
cd rust/snapclone-videosrc
cargo build --release --bin sc-camdiag
# binary at target/release/sc-camdiag
```

Pure std, no dependencies — builds with the stock rustc.

## Run

Get the node name from the app log line `start() ACCEPTED — requested-device:
"..."` (or `pw-dump | grep node.name`). Then:

```sh
sc-camdiag libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29 \
           --width 1280 --height 720 --seconds 10
```

It prints a report ending in a VERDICT line. **Paste the whole report back.**

Example healthy report:
```
──────── sc-camdiag report ────────
frames delivered:     298
capture window:       9.94s
measured fps:         30.0
max inter-frame gap:  41 ms
stalls >200ms:        0
bad-layout frames:    0 / 298
identical consecutive:0
VERDICT: HEALTHY — steady delivery, correct layout.
```

## How to use it to debug

- **VERDICT: HEALTHY but the app preview is still bad** → the problem is in the
  app's processing/gating/display, NOT the pipeline. Next: check the new real
  `frames-seen` count in the app log and whether `PAUSED` lines correlate with the
  freeze. This points at the frame-gate (see below).
- **VERDICT: LOW FPS / stalls** → the pipeline itself is slow on this device;
  investigate the gst pipeline (videoconvert ×2 on CPU, ISP, libcamera).
- **VERDICT: STARVED / NO FRAMES** → node name wrong, permissions, or the source
  stops after a few frames (driver/power issue).
- **VERDICT: LAYOUT ERRORS** → stride/size mismatch → tiling. Capture the report;
  the stride math is in `validate_preview_frame`.
- **VERDICT: POSSIBLE FREEZE** → many byte-identical consecutive frames → the
  source is delivering the same frame (sensor/ISP stuck) rather than the app
  freezing.

## The gate question (likely culprit for "freezes when I click away")

`frame_gate(app_active, widget_visible, screen_off, scan_mode)` currently pauses
the instant `app_active` is false. `frame_gate_debounced(..., ms_since_active,
grace_ms)` keeps the preview alive through a brief inactive blip when the widget
is still visible and the screen is on. Both are tested. **Not yet wired into the
app** — that's a product call: if `sc-camdiag` says the pipeline is HEALTHY while
your preview freezes on focus-loss, wiring the debounce (e.g. 400 ms grace) is the
fix. Tell me and I'll wire it.
