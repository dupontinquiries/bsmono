# Camera tiling + live-preview performance — analysis and a staged GPU plan

Status: **analysis + a tested geometry module** (`snapclone-videosrc::geometry`).
The GPU/zero-copy work is specced in stages; the C++/Vulkan glue is deferred
(untestable without the device + a GL/Vulkan context), but the geometry every
stage depends on is now verified.

## Two distinct problems (don't conflate them)

**1. Tiling** is a *correctness* bug: a sheared, diagonally-walking image. Its
cause is a ROW STRIDE mismatch — the producer (ISP or `gldownload`) pads each
row up to an alignment boundary, so real bytes-per-row = `round_up(width×bpp,
align)`, not the tight `width×bpp` a naive reader assumes. Off by the pad each
row → skew.

**2. Live-preview cost** is a *performance* problem: the GPU path today is
`glupload → glcolorscale → glcolorconvert → gldownload → videoconvert → fdsink`,
then the app reads raw bytes off a pipe and builds a `QImage` per frame. The GPU
does real work and then we **download the result back to system memory** and
copy it again. At 1280×720×4 that's a 3.7 MB download + pipe + heap copy every
frame — the GPU acceleration is half-wasted because the output round-trips
through the CPU.

## What the device logs actually establish

* At 1280×720 the log shows `bytes-per-frame = 3686400 = 1280×720×4` exactly —
  so at this resolution there is **no padding** and tiling won't occur. Tiling
  only appears at widths whose tight row isn't already alignment-sized. (The
  geometry module's `tight_1280_has_no_padding` test pins this.)
* The pipeline already parses the negotiated sink caps and, when present, passes
  the real `bytesPerLine` to the `QImage` — which is the *correct* tiling fix.
  The residual risk is the window before caps are parsed, or producers that
  don't emit a stride.
* The GPU path is selected and works (`Using GPU colorspace path …`), and the
  recording/transcode AV1+ROI path runs end-to-end. So the GL stack is healthy;
  the opportunity is to stop downloading from it.

## Tiling: finish the correctness fix (cheap, partly testable)

1. **Prefer alignment-avoidance for the preview size.** Round the requested
   capture width DOWN to a multiple that makes the tight stride alignment-clean
   (`geometry::align_width_down`). 1280 already is; the policy default should
   keep preview widths on that grid so padding can't arise. (Tested.)
2. **When padding is unavoidable, read at the real stride.** Keep using the
   negotiated `m_sinkStride`. For the pre-caps window, instead of dropping the
   buffer on bloat, use `geometry::best_stride_guess(observed_total)` to recover
   the actual stride and re-slice — turning "reset and hope" into deterministic
   recovery. (`infer_recovers_the_alignment_from_a_bloated_buffer` tests this.)
3. This removes tiling without any GPU change and is the right first step.

## Performance: the staged GPU plan (the Vulkan + Mesa direction)

The goal is to **keep the frame on the GPU** from capture to screen — no
`gldownload`, no pipe, no per-frame `QImage`. Staged cheapest-first:

### Stage A — drop the redundant CPU convert (small, safe)
Where a hardware m2m converter exists (`v4l2convert` on some pmOS builds), prefer
it ahead of GL for pure format conversion — no GL context setup. Add it to the
convert/scale segment preference order. Pure pipeline change, no app code.

### Stage B — in-process GStreamer with `appsink` (medium)
Replace the `gst-launch` subprocess + fd pipe with an in-process pipeline ending
in `appsink`. This alone removes the pipe serialization and lets us pull frames
with their stride metadata intact (no inference needed) — the `QImage` is
constructed from the mapped buffer with the correct `bytesPerLine` directly.
Prerequisite for C, and a meaningful latency win on its own.

### Stage C — zero-copy GPU import (the big win): DMA-BUF → RHI/Vulkan
End `appsink` on `caps=video/x-raw(memory:GLMemory)` (or `memory:DMABuf`), pull
the `GstGLMemory`/`GstVideoMeta`, export a DMA-BUF fd, and import it into Qt's
render path **without ever touching system memory**:

* **Qt RHI (preferred, portable).** Qt 6's scene graph runs on QRhi, which has a
  Vulkan backend on Mesa. Import the DMA-BUF as a `QRhiTexture` via
  `QRhiTexture::createFrom`/native-texture import (Qt 6.6+), wrap it in a
  `QSGTexture`, and draw it in a `QSGSimpleTextureNode`. The geometry module's
  `padded_stride`/`frame_size` describe the imported plane layout to the RHI.
* **Why Vulkan on Mesa specifically.** On the FP5 (Adreno 643 via Mesa/Turnip)
  and on Mesa desktop, the Vulkan path gives explicit control over external-
  memory import (`VK_EXT_external_memory_dma_buf` / `VK_EXT_image_drm_format_
  modifier`) so the camera buffer becomes a sampled image with the producer's
  DRM format modifier — true zero-copy, GPU-tiling-aware (the GPU samples the
  vendor-tiled buffer directly; CPU stride becomes irrelevant). Force Qt's RHI
  to Vulkan (`QSG_RHI_BACKEND=vulkan` or `QQuickWindow::setGraphicsApi`).
* **Per-frame cost** drops from "download 3.7 MB + pipe + QImage + CPU paint" to
  "import fd (cheap, ref-counted) + GPU fence wait + texture bind". This is what
  unlocks 1080p+ at 30 fps on the FP5, and it also makes tiling structurally
  impossible — the GPU honors the buffer's format modifier, so there's no CPU
  stride to get wrong.

### Stage D — sample colorspace/orientation in the shader (free once on GPU)
Once the frame is a GPU texture, do NV12→RGB, rotation, mirror, and the ROI
preview overlay in the fragment shader instead of in GStreamer. Removes
`glcolorconvert`/`videoflip` from the pipeline; it's a few lines of GLSL/SPIR-V.

## Risks / honest caveats

* DMA-BUF import is the hard part: format-modifier negotiation between
  libcamera→GStreamer→Vulkan can fail on a given board, needing a fallback to
  the Stage B (mapped `appsink` + CPU upload) path. Keep that fallback wired.
* Qt's DMA-BUF/native-texture import maturity varies by Qt version and RHI
  backend; this needs on-device validation, not just API presence.
* None of Stages B–D is testable in this environment (no Qt, no GL/Vulkan
  context, no device). They are specced, not written. What IS verified is the
  geometry every stage relies on.

## What ships now (tested)

`snapclone-videosrc::geometry`: `round_up`, `padded_stride`, `frame_size`,
`tight_frame_size`, `infer_stride`, `best_stride_guess`, `align_width_down`,
`StrideCandidate`. 5 tests including the real 1280 (no-pad) case, an
unaligned-width tiling case, alignment recovery from a bloated buffer, and
width-alignment avoidance. This replaces the untested inline heuristic in
GstLaunchBackend and is the shared math for both the tiling fix and the GPU
import layout.
