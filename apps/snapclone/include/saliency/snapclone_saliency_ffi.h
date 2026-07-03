#ifndef SNAPCLONE_SALIENCY_FFI_H
#define SNAPCLONE_SALIENCY_FFI_H

// ─── snapclone_saliency_ffi.h ────────────────────────────────────────────────
//
// C ABI for the Rust snapclone-saliency crate. Available when CMake is built
// with -DSNAPCLONE_RUST_SALIENCY=ON (default ON).
//
// Used by the Wayfinder page to crop contact avatars to the most visually
// salient region instead of blindly center-cropping. Pure-Rust classical
// saliency (no ML, no model files); ~5-15ms per call on FP5.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Salient bounding box in input pixel coordinates.
/// All zeros means: no salient region detected (caller falls back to center crop).
typedef struct {
    uint32_t x;
    uint32_t y;
    uint32_t w;
    uint32_t h;
} SalRect;

/// Compute the bounding box of the most salient region.
/// `rgba` must point to at least width*height*4 bytes (RGBA8888).
/// Pointer is borrowed; the function reads but does not retain it.
SalRect sc_saliency_bbox(const uint8_t *rgba, uint32_t width, uint32_t height);

/// V32.32: compute a landmark-candidate score in [0, 1] for the given frame.
/// Higher score = better candidate to suggest "save as landmark" for. The
/// score blends four heuristics from one shared Sobel pass:
///   - Strong horizontal edge (skyline / horizon)
///   - Edge-density variance across 3×3 zones (distinctive subject)
///   - Vertical-edge dominance (architectural structure)
///   - Clean-line vs noisy-texture ratio (rejects crowds, screens, foliage)
///
/// Designed to be called periodically (~1Hz) from the camera widget on the
/// live preview frame. Caller should pre-downsample to ~256×256 if the source
/// is larger; computation cost scales linearly with pixel count for the input
/// pass and stays constant for the GRID-internal work.
///
/// Returns 0.0 on invalid input (null pointer, oversize dimensions).
float sc_landmark_score(const uint8_t *rgba, uint32_t width, uint32_t height);

/// V32.34+ ROI bit-allocation plan for face-aware video compression.
///
/// A directive is a rectangle + a QP delta to apply inside it. qp_delta_centi
/// is the quantizer offset in 1/100ths: NEGATIVE = more bits (protect/sharpen),
/// POSITIVE = fewer bits (compress harder). Divide by 100.0 for ffmpeg
/// `addroi`'s rational qoffset in [-1, 1].
typedef struct {
    uint32_t x;
    uint32_t y;
    uint32_t w;
    uint32_t h;
    int32_t  qp_delta_centi;
} CRoiDirective;

/// Turn a saliency bounding box into a face-aware bit-allocation plan: a
/// background penalty over the whole frame (compress harder where the viewer
/// isn't looking) plus a PADDED face boost that protects the subject from that
/// global degradation. Net: bits move from background to face → smaller file at
/// equal perceived quality, the actual point of face-aware compression.
///
/// The caller passes a fixed-capacity `out` array (cap directives). We fill up
/// to `cap` and return the count written (0 = no usable ROI → encode plainly).
/// Directives are applied IN ORDER, later overriding earlier where they overlap
/// (matches an ffmpeg addroi filter chain): index 0 = background, last = face.
///
/// `out` must point to at least `cap` writable CRoiDirective slots. A capacity
/// of 2 is sufficient for the current planner (background + face).
uint32_t sc_roi_plan(uint32_t bbox_x, uint32_t bbox_y,
                     uint32_t bbox_w, uint32_t bbox_h,
                     uint32_t frame_w, uint32_t frame_h,
                     CRoiDirective *out, uint32_t cap);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // SNAPCLONE_SALIENCY_FFI_H
