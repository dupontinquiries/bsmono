# Getting camera frames on a Fairphone, in Linux

Verified against **GNOME Snapshot** (`snapshot-main`), which is the reference
camera app for mobile Linux and ships on this class of device. What follows is
read from its source, not inferred.

## The route is the portal, not the device node

Snapshot never opens `/dev/video*` and never talks to libcamera directly. It
asks the **XDG desktop portal**, which hands back a **PipeWire file
descriptor**:

```
org.freedesktop.portal.Camera
        │
        ├── request_access()          ← the permission prompt
        ├── is_present()              ← is there a camera at all
        └── open_pipe_wire_remote()   ← returns an OwnedFd
                    │
                    ▼
              PipeWire remote
                    │
                    ▼
              pipewiresrc / node
```

From Snapshot's `stream()`:

1. `camera::Camera::new()`
2. `request_access(CameraAccessOptions::default())`
3. `is_present()`
4. `open_pipe_wire_remote(...)` → `OwnedFd`

**The order matters and is not decorative.** `is_present` is checked *after*
access is granted, because on a device with no camera the access request still
succeeds — so a build that skipped the presence check would open a remote with
no nodes on it and report the failure as a stream error rather than as "this
machine has no camera".

`ashpd` is the crate (`features = ["camera"]`), and Snapshot also calls
`ashpd::register_host_app(APP_ID)` so the portal can attribute the prompt.

### This is what makes a Flatpak build work

Inside a sandbox there is no `/dev/video*` to open. The portal route is the
only one that works both sandboxed and not, which is why
[`camera_select`](../egui-frontend/src/camera_select.rs) is written against
**PipeWire node properties rather than device paths**: a portal-provided node
arrives with the same properties through a different route, so it needs no
second code path.

## The properties are the ones we already read

`aperture::Camera::location()` reads **`api.libcamera.location`** — the same
key `camera_select::device_from_node` uses.

**One correction landed from this reference.** Aperture matches three spellings
per side:

```
"Front" | "front" | "0"  → Front
"Back"  | "back"  | "1"  → Back
_                        → Unknown
```

Our port matched lowercase only. libcamera reports the capitalised and numeric
forms on some pipelines, so a capitalised `Front` fell through to guessing from
the node description — **a silent downgrade from a stated fact to an
inference**, and the visible symptom is an un-mirrored selfie. Fixed, with a
test over all six spellings.

An *unrecognised* location is now `Unknown` rather than a label guess: the
platform stated something and we failed to read it, which is worth being
unknown about rather than papering over.

## What Snapshot does not answer

* **Error handling has one case worth copying.** Snapshot matches
  `PortalError::NotAllowed` specifically — a denied permission is a different
  UI from a broken pipeline, and conflating them tells a user to check their
  hardware when they need to check their settings.
* It uses GStreamer (`camerabin`, `pipewiresrc`) for the pipeline. Our port does
  not have to: the portal gives a PipeWire fd, and what consumes it is open.
* It has no equivalent of our **capture-group** constraint, because it shows one
  camera at a time. The FP5 ISP limit that breaks PIP is ours to carry.

## Open on our side

The `pw-stream` binding that turns the fd into frames. Everything it would
*decide* — which node, which way up, whether PIP is possible — is already
decided and tested in `camera_select` without a device.
