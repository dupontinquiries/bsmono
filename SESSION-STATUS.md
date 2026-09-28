# What was done, what was run, what was not — 2026-09-27

Five apps touched. Every claim below is either "tests pass" (run here) or
"seen in a screenshot" (Xvfb + xdotool, `tools/shot.sh`; images in `shots/`).
Anything needing hardware, a network peer or an account is marked NOT RUN.

## bo-suite-egui (music player)
Run: 625 lib tests + 80 bo-media tests pass. Real mpv playback (null audio
output) with JSON-IPC steering: play / pause / resume / seek / next / previous /
queue reorder / volume (pactl), session save + restore, Home lists, search,
albums/artists/tags/playlists, compact reflow at 420 px. Screenshots bo-00..06.
Fixed on the way: `exec` before env assignments (player never started),
PulseAudio media.name via `env`, quoted-path parsing, mpv term-status → IPC,
BEGIN IMMEDIATE migration race, double-counted plays, bottom bar swallowing the
page when the queue opened.
NOT RUN: a real sound card, the ANC hardware path, Bluetooth.

## snapclone-egui
Run: 1791 lib tests (+3 +1) pass. Two instances on one host: QR friend add,
vCard import, photo snap end to end (persisted inbox, restart, viewer with the
picture), video clip end to end (spill → libsvtav1 → send → open). Screenshots
snap-01..07. Fixed: two instances on one port (SO_REUSEPORT), roster dedup by
sc-id, second clip failing on `0.mkv`, false "incoming call" from a NAT punch.
NOT RUN: real camera, microphone/voice audio, a second physical device.
Known gap: snap transport is plaintext TCP while the summary says encrypted.

## voiceterm-egui
Run: 2021 lib tests pass. Push-to-talk → `voice_router` chain → staged shell
line (never executed), "open mail" by voice, Ctrl+Space / Ctrl+Shift+P, Proton
sign-in worker (SRP proofs verified offline against proton-srp's own server,
TOTP continues the same session). Screenshots vt-01..05.
NOT RUN: a live Proton login (the sandbox TLS proxy rejects the certificate —
UnknownIssuer — so the worker was seen reaching the network and failing
honestly, nothing more), a real microphone, wake word, the F1 overlay.

## ambient-learner-egui
Run: 296 egui-frontend tests pass headless; the GTK binary still builds and its
131 tests pass after the engine extraction. The engine (`src/engine.rs`: Model,
ThermalPoller, Database, locations, device state) is now shared by both
frontends; the egui app runs the same WorldState worker threads as GTK and
shows real numbers. Walked with a fake sysfs (`AMBIENT_SYSFS_ROOT`): three
sensors → prediction → calibrate ×3 → model + neural field saved → DB rows →
History week chart + CSV export → Places add/select (persisted) → Model log →
Sync export → Debug page with the published screen-temp frame and the host
log. Screenshots al-01..09.
Fixed on the way (both frontends): `latest_database_in` matched
`.db.backup`/`-wal`/`-shm` (every second launch opened the backup);
`get_calibration_history` selected a column that never existed; page
shortcuts fired while typing in a text field.
Observed, not changed: the linear model's LR (0.01) moves the prediction
slowly — four calibrations to 21.5 °C took it from 0.0 to 6.1 °C. Same engine
as GTK. `get_device_state` reports 50 % battery when there is no battery.
NOT RUN: real thermal zones, IIO sensors (pressure/humidity/light), WiFi
location detection, the D-Bus service.

## photosync-egui (new crate: apps/photosync/egui-frontend)
Run: 35 core + 38 frontend tests pass. Folder scan on a thread with an
index + thumbnail cache (second launch restores from cache), day grid, viewer
(swipe / arrows / tap-to-page, full decode in the background), tags (persisted),
"find similar" (core `best_matches`), duplicates page with keep-one → trash
folder, per-contact revocable sharing (grants persisted; contacts read from
snapclone's `contacts.conf`), library page with rescan and folder change.
Screenshots ps-01..09 against a synthetic 15-photo library with real
duplicates (copy, re-encode, resize).
Core fixes: `tag_similarity` now applies `AUTO_TAG_WEIGHT`; dedup adds a tone
check because a gradient and a flat colour share a zero dHash.
NOT RUN: a real camera roll, EXIF (dates are file dates and the UI says
"modified"), byte delivery to a contact (the grant is kept; snapclone is the
transport and the page says so).

## snapshot-cam (the standalone camera) — added 07:15–07:50
Run: 35 headless tests (roll, store, camera rig on the test pattern, both
handoffs), 12 in the new `shared/capture-kit`, 5 in `shared/snap-handoff`.
The stub became a camera: capture-kit (snapclone's engine hoisted) feeds the
viewfinder, the shutter writes JPEGs and a persisted roll, "back up" copies
into photosync's library (photosync's rescan showed it), "send to Bee" goes
through snapclone's outbox and came back as "shared with Bee" after snapclone
A delivered it to snapclone B's inbox — the whole loop on one host, shots
sc-01..09. Contacts from snapclone's book.
snapclone changes: the outbox pump (waits until the peer has been heard this
session AND one of our announce ticks has gone out — the first attempt was
refused on the far side as "not a known contact" because it fired at t=2s);
the per-recipient send extracted into `dispatch_snap` so both paths share it.
QR: the camera page's "scan" is now the popup the Friends page opens, and the
popup shows the LIVE picture in its well above every control (it was an empty
square; the old in-place toggle throttled the camera and ran no decoder).
Caption fixed: the code is scannable by another snapclone, not by a stock QR
reader. Verified: B's popup decoded A's code from the live frame (log:
"scanned sc-8fd30abd").
Later the same day: GStreamer installed in the sandbox, so the gst-launch
path is RUN now — capture-kit's `real_gstreamer_delivers_frames_through_the_fdsink_path`
(videotestsrc through the same fdsink framing a camera uses; skips loudly
where gst is absent), snapshot-cam with `SNAPSHOT_CAM_SOURCE="gst:videotestsrc
pattern=ball"` (shot sc-10: 1280×720 JPEG from a real pipeline), and snapclone
with `camera.device = "gst:videotestsrc pattern=smpte"` (shot sc-13). snapclone's
own capture.rs now re-exports the kit (only its QR test pattern stays local;
1785 lib tests). snapshot-cam also gained: a viewer (tap a roll row: full
photo, swipe/arrows, delete to `.trash/`), a sticky "sent to" record so a
back-up after a send still says who has the photo, remembered facing/fill,
a thumbnail decode budget, a 3s/10s self-timer with a countdown over the
viewfinder (shot sc-14), layout tests (shell moved into the library), and
.desktop entries for it and photosync-egui. 42 tests.
STILL NOT RUN: a real camera SENSOR (v4l2src/pipewiresrc need hardware).

## Digital zoom with pinch, both camera apps — added afterwards
`shared/uikit` gained `zoom::Zoom` (13 tests): a window into the frame in
frame fractions, pinched about the point between the fingers, panned by a
one-finger drag while zoomed, clamped to the frame, stepped by keys, with a
chip that resets. Input is the kit's existing touch bridge, so a two-finger
pinch, a trackpad pinch and ctrl+wheel are one `Gesture::Pinch`. capture-kit's
`RawFrame::crop` (3 tests) saves the window at native pixels — no upscaling;
a 2× photo of 1280×720 is 640×360.
snapshot-cam: pinch/ctrl+wheel/`+`/`−`/`0`, chip, mirror-aware (a pinch on the
mirrored preview's left zooms into the frame's right), the shutter crops.
44 tests, incl. an end-to-end one on the test pattern (Zoom event → 2× →
the JPEG on disk is 320×180). Shots sc-15 (4.0× chip), sc-16 (2.2× + a mouse
pan + a shot: a 576×323 file, roll row "saved … (2.2× crop, 576×323)").
snapclone: `Viewfinder.zoom` composes with the mirror and the quarter turns
(window → mirror → turn; tap and reticle round-trip through all three, tested
for every combination), pinch only while the frame is live (not while
composing, not under the QR popup), `HostCommand::CaptureSnap { crop }`
carries the window and the host crops before encoding. Two fingers no longer
drag the page: egui reports the first finger as the pointer, so a pinch used
to begin a page swipe — gated in `tick_page_swipe`, with a test that fails
without the gate. 1798 lib tests. Shots snap-zoom-1 (3.3×), snap-zoom-2
(2.2× shutter → Compose; the file `snap-zoom-2-file.jpg` is 287×216 of the
640×480 gst frame), snap-zoom-3-focus (reticle under a tap while zoomed).
FOUND AND FIXED on the way: tap-to-focus (and double-tap flip) never fired
from a real pointer — `tick_feedback` released the press machine at the top
of every frame and discarded the `Tap` before the camera page read it. Now
kept for the page and cleared at the end of the frame. The reticle also drew
at a fraction of the VIEW (wrong for a mirrored or turned preview, and for a
zoomed one); it now draws through `Viewfinder::to_screen`.
Pre-existing, not mine, left alone: uikit-egui's
`callout::body_text_stays_readable_on_every_tinted_surface_in_every_palette`
fails on the "slate" palette's Error tint (2.93:1) — callout.rs and the
theme crate are byte-identical to the upload; the other 568 kit tests pass.
NOT RUN: an actual two-finger pinch — no touchscreen here. The two-finger
path is the touch bridge, exercised with synthetic `Event::Touch` pairs in
the snapclone gate test and in touchkit's own tests; the pointer-side path
(ctrl+wheel) is what the screenshots used.

## snapshot-cam: DSLR controls, lens profiles, autocrop, focus per lens
Run: 84 tests (was 44). New headless modules: `develop` (one linear-light
resampling pass: Brown distortion, lensfun-style vignetting, crop, zoom,
anamorphic desqueeze, WB, EV; histogram/meter/zebra/peaking), `lens`
(INI profiles, built-ins for Moment's line, black-border autocrop with
deadband + inset, per-lens focus hint), `hwctl` (V4L2 controls via
`v4l2-ctl`: exposure time, gain, WB temp, focus, their automatics; driver
name aliases), `pro` (settings → render params), `developer` (preview
develop on a worker thread, latest wins). Shell: "pro ▸" panel, aids over
the viewfinder, the shutter renders the same pass at native pixels.
Seen under Xvfb through real gst-launch-1.0 with a fisheye-style still
(scene in an image circle on black, warm cast): shot sc-17 (autocrop +
auto WB), sc-18 (panel), sc-19 (+1 EV, zebras, peaking, meter +1.5,
clipped 41%, crop locked, profile saved). Saved file 592×336 = the
native-pixel rectangle inside a 340 px circle.
Fixed on the way: two photos in the same second wrote one file (the
second silently replaced the first) — `store::free_name`.
Timing: preview develop 640×271 is ~18 ms (+4 ms aids) release on this
x86 box, ~170 ms debug — why it moved off the UI thread.
NOT RUN: a real V4L2 camera for the hardware controls (parse and command
tested against captured C920 output and a stand-in script); real Moment
lenses (built-in coefficients are zero by design, not measured); an ARM
timing. PipeWire/libcamera cameras have no V4L2 controls, and the panel
says so. Frames are 8-bit post-ISP, not RAW.

## snapshot-cam: triggers, merges, cover, layout
Run: 107 tests. New headless modules: `triggers` (evdev volume/power
keys via /proc/bus/input/devices bitmaps, EVIOCGRAB for button-only
devices, logind inhibitor for the power key; IIO accelerometer + shake
detector), `prime` (sharpest frame in the second after a trigger),
`stack` (translation alignment, Mertens exposure fusion, focus stacking,
linear burst average; pyramid blending), `bracket` (control-stepping
sequencer with settle frames and restore; HDR/focus/burst/WB bracket),
`cover` (grey-card calibration of tint gains + flare). Shell: shots,
trigger, cover and view rows; full-screen / compact / auto layout.
Seen through real gst-launch-1.0: sc-20 (full screen, fisheye autocrop),
sc-21/22 (cover before/after: measured haze 5.95/5.45/4.52% vs the 6.0/
5.5/4.5% simulated, gains R×0.90 B×1.37 vs 0.90/1.36), sc-23 (burst
merge through the shutter, a complete 1280×720 JPEG).
End-to-end test on the test-pattern camera: shake → primed → saved;
burst → merge worker → saved; WB bracket → three files, bluest first.
Fixed on the way: a JPEG killed mid-write stayed on the roll truncated
(found by killing the app the instant a merge's file appeared) — now
encoded in memory, written to a hidden temp, fsynced, renamed.
NOT RUN: real volume/power keys, the grab ioctl and logind (no input
devices or logind here; discovery, bitmaps, event decoding and the reader
thread are tested on captured text and files); a real accelerometer (the
detector is tested on synthetic shakes, bumps and walking); HDR and focus
brackets against a real camera (the sequencer is tested with a captured
C920 control set; the merges on synthetic scenes). Alignment is
translation only; moving subjects ghost in HDR.

## Version gotchas hit
- egui 0.34 `zoom_delta()`: a trackpad pinch, ctrl+wheel and `Event::Zoom`
  all land in it; two fingers come through `multi_touch()` instead and the
  first finger is ALSO the pointer (down, moving) — anything reading
  `pointer.primary_down()` as a one-finger drag must check `multi_touch()`.
- Overlapping `ui.interact` widgets: the one registered LAST is on top. A
  chip drawn inside a viewfinder function that a later full-page
  `interact` covers never gets its click; draw it after.
- xdotool `click 1` is press+release inside one frame: a widget that needs
  to SEE the press (`is_pointer_button_down_on`) never does. Use
  `mousedown 1 sleep 0.1 mouseup 1` in one xdotool chain (shot.sh's
  per-line 0.6 s pause turns separate lines into a 500 ms+ hold).
- egui 0.34: `TopBottomPanel` → `Panel::bottom`; `wants_keyboard_input` →
  `egui_wants_keyboard_input`; `CentralPanel::show` deprecated for
  `show_inside` (left as the siblings do); `rect_stroke` takes `StrokeKind`.
- A `right_to_left` layout directly in a vertical Ui consumes all available
  height in a bottom panel — wrap it in `ui.horizontal`.
- Cargo ignores `required-features` on `[lib]`; doctests in a binary crate
  never ran and fail once the same file is under a lib root (`doctest =
  false` on the shared-engine libs).
- dash drops env assignments with dotted names (`PULSE_PROP_media.name`);
  route them through `env`.
- `image` 0.25's JPEG encoder refuses RGBA.
