#pragma once
// ─── UI/camera/FramePixelOps.hpp ──────────────────────────────────────────────
//
// V32.39+ : shared, SIMD-accelerated pixel operations used by the CPU paths of
// ALL camera backends. Before this, each backend hand-rolled its own copy of the
// same per-pixel work — GstLaunchBackend used QImage::rgbSwapped() (allocates a
// whole second image), QtCameraBackend had an inline scalar NV12/NV21/I420→RGB
// loop, LibcameraBackend leaned on QImage format reinterpretation. This unifies
// the hot loops in one place so they can be optimized once and reused.
//
// Dispatch is resolved ONCE at first use:
//   • aarch64 (FP5, Asahi) → NEON              (ARMv8 always has Advanced SIMD)
//   • x86-64 desktop       → AVX2 ▸ SSSE3      (runtime __builtin_cpu_supports)
//   • anything else        → portable scalar
//
// SAFETY — because NEON cannot be compiled on the (x86) build host, a built-in
// self-test runs the selected backend against the scalar reference on a small
// synthetic image at init; on ANY mismatch it permanently downgrades that op to
// scalar and logs once. So an un-host-verified SIMD path can never render wrong
// colors — at worst it falls back. If the aarch64 build ever fails to COMPILE
// the NEON block, build with -DSNAPCLONE_PIXELOPS_SCALAR_ONLY to force scalar.
//
// All functions are row-stride aware and tolerate widths that aren't a multiple
// of the SIMD width (scalar tail). They never read/write row padding beyond the
// active width.
// ─────────────────────────────────────────────────────────────────────────────

#include <cstdint>

namespace SnapClone::Camera::PixelOps {

/// In-place R↔B swap for packed 32-bpp (BGRA↔RGBA / BGRX↔RGBX). The 4th byte
/// (alpha / X) is left untouched. `stride` is bytes-per-row (≥ width*4). This is
/// the in-place, allocation-free replacement for QImage::rgbSwapped().
void swapRB8888(uint8_t *data, int width, int height, int stride) noexcept;

/// Semi-planar 4:2:0 → packed RGB888 (byte order R,G,B). BT.601 limited-range,
/// matching the integer coefficients the backends already used. `nv21=false` is
/// NV12 (chroma byte order U,V); `nv21=true` is NV21 (V,U). The chroma plane is
/// half height; the caller passes the full-resolution width/height.
void nv12ToRgb888(uint8_t *dst, int dstStride,
                  const uint8_t *yPlane, int yStride,
                  const uint8_t *uvPlane, int uvStride,
                  int width, int height, bool nv21) noexcept;

/// Planar 4:2:0 (I420 / YUV420P, separate U and V planes) → packed RGB888.
void i420ToRgb888(uint8_t *dst, int dstStride,
                  const uint8_t *yPlane, int yStride,
                  const uint8_t *uPlane, int uStride,
                  const uint8_t *vPlane, int vStride,
                  int width, int height) noexcept;

/// Name of the active accelerated backend: "NEON", "AVX2", "SSSE3", "scalar", or
/// "scalar (simd self-test failed)". Stable after first use; cheap. Intended for
/// a one-line log in each camera backend so the journal shows what's in play.
const char *backendName() noexcept;

/// Force the init + self-test now (otherwise it runs lazily on first op). Returns
/// true if the selected SIMD backend matched the scalar reference. Idempotent and
/// thread-safe.
bool selfTest() noexcept;

} // namespace SnapClone::Camera::PixelOps
