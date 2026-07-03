// ─────────────────────────────────────────────────────────────────────────────
// FocusSweep.hpp — sharpness metric + CDAF (contrast-detect autofocus) sweep
//
// V32.32: standalone algorithm code with no Qt dependency beyond QImage for
// the sharpness input. Callers (CameraWidget) own a VcmController + a way
// to get a frame; they invoke runCdafSweep(vcm, getNextFrame) and get back
// the best focus position.
//
// Algorithm:
//   - Coarse sweep: 9 steps across [min, max], measure sharpness at each.
//   - Pick the step with peak sharpness.
//   - Fine sweep: 11 steps in [±range/8] around the coarse peak.
//   - 3-point parabolic refinement on the fine peak to find a sub-step
//     optimum.
//   - Move to that final position, return DAC value.
//
// Sharpness metric: Laplacian variance on the green channel of the centre
// 1/3 ROI. Higher = sharper. Green is used because Bayer sensors have 2x
// green pixels (less noise) and most colour information is correlated
// with luminance.
// ─────────────────────────────────────────────────────────────────────────────

#pragma once
#include <QImage>
#include <functional>

namespace SnapClone::UI::Camera {

class VcmController;

/// Compute Laplacian variance on the green channel of the centre ROI.
/// Higher = sharper. Returns 0 if image is too small or empty.
/// Works on QImage::Format_RGB888 / RGBA8888 / RGB32 — auto-converts.
double laplacianSharpness(const QImage &img);

/// Result of a CDAF sweep.
struct CdafResult {
    int    finalPos     = 0;       ///< DAC position the lens ended up at
    double finalSharpness = 0.0;   ///< sharpness at finalPos
    bool   succeeded    = false;   ///< false if frames couldn't be acquired
};

/// Synchronous CDAF sweep. Drives the VCM through positions, asks the
/// caller for a settled frame at each position via getNextFrame, and
/// returns the best position found.
///
/// getNextFrame is called with NO arguments and must return the most
/// recent settled frame (caller is responsible for any settle delay —
/// see runCdafSweepWithSettle below).
///
/// Returns CdafResult with succeeded=false if any frame fetch returned
/// a null QImage.
///
/// SLOW MODE: 9 coarse + 11 fine + parabola refinement. ~3 seconds with
/// 150ms settle. Use for a "Focus" button or "long press" interaction.
CdafResult runCdafSweep(
    VcmController &vcm,
    std::function<QImage()> getNextFrame,
    int settleMs = 150);

/// Fast CDAF sweep for tap-to-focus. Only 5 coarse steps, no fine pass,
/// no parabola refinement. ~750ms total at 100ms settle. Less accurate
/// but feels responsive for interactive use. Returns the best coarse
/// position.
///
/// V32.35+: `isCancelled`, if provided, is polled before each lens move. When
/// it returns true the sweep aborts immediately WITHOUT moving the lens
/// further and returns succeeded=false. This lets a double-tap-to-flip bail out
/// of an in-flight focus before any (further) motor motion — the "analyze
/// cheaply, only commit the motor if still wanted" pattern. The predicate is
/// called from the worker thread, so it must be thread-safe (e.g. read an
/// std::atomic<bool>).
CdafResult runFastFocus(
    VcmController &vcm,
    std::function<QImage()> getNextFrame,
    int settleMs = 100,
    std::function<bool()> isCancelled = {});

} // namespace SnapClone::UI::Camera
