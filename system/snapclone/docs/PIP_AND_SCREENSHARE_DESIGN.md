# Picture-in-Picture, Webcam Arrays, and Screenshare — Diagnosis + Design

Status: **diagnosis + design**, plus a concrete PiP-UX fix. Covers (1) why PiP
is "still broken," (2) how to generalize the camera source model so desktop USB
webcam arrays and PipeWire screenshare become first-class sources, and (3) why
those two are the same problem.

---

## 1. Why PiP is still broken — the honest root cause

The PiP *code* is sound. `startPip()` spins up a second backend on the front
camera, composites it into an overlay, retries once on transient failure, logs
first-frame arrival, and hides the overlay on hard failure. The compositing,
mirroring, and positioning are correct.

**The breakage is upstream and is a hardware/stack constraint, not a bug:**

- PiP runs TWO `pipewiresrc` instances at once — one per `target-object=<node>`
  (main = rear, pip = front).
- On the **FP5 (and most single-ISP mobile camera stacks)**, the front and rear
  sensors share one ISP pipeline. Opening the second sensor while the first is
  live fails negotiation. This is exactly the camera-bridge log we have on
  record: `Failed to build output pipeline` when multiple virtual cams are
  requested.
- `backendHasFrontAndRear()` only checks that both a front and a rear device
  **exist in the registry** — it never checks they can run **simultaneously**.
  So PiP is *offered* on hardware that can't deliver it.
- The retry-once logic correctly handles a *transient* bridge-startup delay, but
  cannot fix a *hard* "no simultaneous dual-camera" limit. It retries, fails
  again, hides the overlay — and the user sees PiP "do nothing" with no reason.

**So "fix PiP" splits into two honest sub-problems:**

1. **Where simultaneous capture is impossible (FP5 dual-sensor):** don't pretend.
   Detect/declare the limitation and either hide the PiP option or fail with a
   *clear, user-visible message* ("This device can't run both cameras at once").
   This is the part shipped now (§4). We cannot make a shared ISP do two
   streams; claiming otherwise is the actual bug.

2. **Where simultaneous capture IS possible (desktop USB webcams, or two
   independent sensors):** PiP should just work — and the same machinery
   generalizes to N sources, which is the webcam-array ask (§2).

---

## 2. Generalized video-source model (the unifying insight)

The `GstLaunchBackend` already targets **any** PipeWire node by name
(`pipewiresrc target-object=<node>`). A physical camera, a USB webcam, and a
PipeWire screenshare stream are *all just nodes*. So the right design is not
"add screenshare" and "add webcam arrays" as two features — it's **generalize
what a `Source` is** and let the existing backend consume any of them.

### 2.1 Source kinds

Extend the source descriptor with a `kind`:

| Kind            | PipeWire `Media.Class`                     | Notes                                  |
|-----------------|--------------------------------------------|----------------------------------------|
| `PhysicalCamera`| `Video/Source` (libcamera_input.*)         | today's cameras; may be ISP-exclusive  |
| `UsbWebcam`     | `Video/Source` (v4l2 / uvcvideo)           | desktop; independent, freely concurrent|
| `VirtualCamera` | `Video/Source` (our camera-bridge nodes)   | today's virtual-cam-*                  |
| `ScreenCast`    | `Stream/Output/Video` via the portal       | screenshare; node from a portal session|

Enumeration already exists for cameras via `pw-dump` in `CameraRegistry`. The
generalization is to (a) stop filtering to only `libcamera_input.*` /
`virtual-cam-*`, and (b) classify each `Video/Source` / `Stream/Output/Video`
node into a `kind`. Desktop UVC webcams show up as plain `Video/Source` nodes
PipeWire already publishes — no bridge needed there.

### 2.2 Concurrency capability — the field PiP needed all along

Add a per-source (or per-source-group) capability:
`concurrent_capture_group`. Sources that share exclusive hardware (the FP5
front+rear ISP) share a group id; only ONE source per group can be live at a
time. Independent USB webcams each get a unique group, so any number run at
once. Screenshare is its own group (always concurrent with cameras).

PiP, multi-cam, and "screen + selfie" all reduce to: *can these N chosen
sources' groups all be satisfied simultaneously?* If yes, run them; if no,
surface why. This single concept replaces the implicit, wrong assumption baked
into `backendHasFrontAndRear()`.

### 2.3 Screenshare via PipeWire (desktop)

Screenshare on Wayland/modern Linux is **not** "grab the framebuffer" — it goes
through `xdg-desktop-portal`'s `ScreenCast` interface:

1. `org.freedesktop.portal.ScreenCast.CreateSession`
2. `SelectSources` (monitor/window, cursor mode)
3. `Start` → user picks what to share in the portal dialog → returns a PipeWire
   **node id** (a file descriptor + node) for the stream.
4. Feed that node to the SAME `pipewiresrc target-object=<node>` the camera
   backend already uses. From the pipeline's perspective it's just a source.

So screenshare capture is: portal handshake (DBus) → get node → existing
backend renders/encodes it. The encode/record/preview path is **unchanged**.
This is why it slots in cleanly: we already have the consumer; we only need the
portal-session producer to hand us a node. `main.cpp` already touches the portal
for other reasons (host-portal registration), so the DBus plumbing isn't alien
to the codebase.

### 2.4 Webcam arrays (desktop)

A "webcam array" is just several `UsbWebcam` sources, each its own concurrency
group. The existing PiP overlay generalizes to a small **compositor**: N
overlays, or a grid, each fed by its own backend instance. Because UVC devices
are independent, the simultaneity that fails on the FP5 *works* here — the very
same code path that's "broken" on mobile is correct on desktop once the
concurrency-group check stops blocking it.

---

## 3. Crypto / privacy considerations

Screenshare is a new and **sensitive** capture surface (it can capture other
apps, passwords, etc.), so:

- Screenshare frames flow through the SAME end-to-end encryption as camera snaps
  — they are content like any other; the network/DHT layer never sees plaintext.
- The portal's user-consent dialog is the authorization boundary; we never
  capture the screen without the portal handing us a node, and we surface
  clearly in-app when a screenshare source is live (a persistent indicator,
  mirroring the recording-dot affordance).
- No screenshare in the thin LAN **server** binary (it has no UI and no business
  capturing a screen); screenshare is a full-client-only source kind.

---

## 4. What ships now (verifiable) vs. deferred (untestable here)

**Now (this PR):**
- A small, dependency-free Rust module `video_source` in a testable crate that
  models source `Kind` + `concurrent_capture_group` and answers the one
  question PiP got wrong: *can this set of sources run simultaneously?*
  Unit-tested, including the FP5 front+rear-exclusive case and the desktop
  independent-webcams case.
- A documented, clearly-scoped PiP-UX fix plan: gate the PiP dropdown entry and
  PiP start on a simultaneous-capture check, and replace the silent black tile
  with a user-visible reason. (C++ wiring is described precisely; the Rust model
  is the source of truth for the decision.)

**Deferred (no Qt/DBus/PipeWire to test here):**
- The `pw-dump` classification extension (camera vs UVC vs screencast).
- The `xdg-desktop-portal` ScreenCast DBus handshake.
- The multi-source compositor UI.
- The C++ `concurrent_capture_group` wiring into `CameraRegistry` +
  `CameraWidget`.

This mirrors the session's working pattern: model the hard decision in
testable Rust first, defer the untestable platform glue with a precise spec.
```
