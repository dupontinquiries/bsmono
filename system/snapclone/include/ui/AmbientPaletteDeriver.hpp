#pragma once
// ─── UI/AmbientPaletteDeriver.hpp ────────────────────────────────────────────
//
// V32.29: derive a Theme::Palette from ambient sources.
//
// MODE A (inbox): pass a list of image file paths (inbox photo thumbs). The
// deriver edge-samples each, mixes by saturation buckets, and returns a
// Palette whose accent + background tones reflect the dominant non-grey
// colors found across the inbox.
//
// MODE B (camera): planned. Same pipeline, fed by viewfinder QImage frames.
// Not yet wired — covered by a separate session.
//
// Output strategy: only a handful of palette slots are overwritten with
// ambient colors. Text/danger/success/border slots are preserved from a
// "base" palette to keep the carefully-tuned relationships intact. The user
// gets ambient *flavour* over a known-good chassis, not a chaotic full repaint.
//
// Throttling and lifecycle are caller responsibilities — this header is
// pure: input list → output Palette in one synchronous call.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/Theme.hpp"

#include <QString>
#include <QStringList>
#include <QColor>

namespace SnapClone::UI {

class AmbientPaletteDeriver {
public:
    /// Derive a Theme::Palette from a list of image paths (Mode A: inbox).
    /// Each path is opened, downscaled to ~64x64, and the edge bands (top,
    /// bottom, left, right — ~6% width each) are mixed into the sample.
    /// The center is deliberately ignored (anti-saliency heuristic — central
    /// subjects shouldn't dominate the ambient palette; backdrops should).
    ///
    /// `base` provides the slots we don't overwrite (text colors, semantic
    /// states, borders). The returned Palette is derived from base with
    /// ambient overrides on BG_*, ACCENT_*, GRAD_*, GLOW_*.
    ///
    /// Returns `base` unchanged if no usable images are found. Callers can
    /// detect that via the unchanged ACCENT pointer.
    static Theme::Palette fromImagePaths(const QStringList &paths,
                                          const Theme::Palette &base);

    /// Derive a Palette from a single live QImage (Mode B: camera).
    /// Same pipeline as fromImagePaths but accepts in-memory frame data.
    /// Designed for the live-viewfinder use case — CameraWidget hands us
    /// every Nth frame and we re-derive the palette. The caller is
    /// responsible for throttling (don't call faster than 1 Hz) and for
    /// applying temporal smoothing across calls if jitter is visible.
    ///
    /// Returns `base` unchanged if the frame is empty or all-neutral.
    static Theme::Palette fromImage(const QImage &frame,
                                     const Theme::Palette &base);

    /// V32.30 (Wave 2): expose the raw {accent, background} colors a
    /// frame would yield, WITHOUT wrapping them into a full Palette.
    /// Used by AmbientCameraDriver to apply exponential-moving-average
    /// smoothing on the underlying signal across frames, then call
    /// fromColors() once with the smoothed result.
    ///
    /// Either color may be invalid (QColor::isValid() == false) if its
    /// bucket was empty in the sample. Callers should handle gracefully.
    struct AmbientColors {
        QColor accent;     ///< averaged saturated edge pixels
        QColor background; ///< averaged neutral edge pixels
    };
    static AmbientColors deriveColors(const QImage &frame);

    /// V32.30 (Wave 2): assemble a Palette from already-chosen accent
    /// and background colors (skip the sampling pipeline). Used after
    /// EMA smoothing to bypass re-derivation.
    ///
    /// Either color may be invalid — corresponding palette slots will
    /// fall back to base. Same slot-override policy as the other
    /// entry points: BG_*/ACCENT_*/GRAD_*/GLOW_ONLINE are written;
    /// text/danger/border/etc inherit from base.
    static Theme::Palette fromColors(const QColor &accent,
                                      const QColor &background,
                                      const Theme::Palette &base);

    /// Derive a Palette from a single QColor accent + base palette. Used
    /// when ambient sampling produced one dominant color (e.g. user-set
    /// per-contact theme, or when image sampling collapsed to a single
    /// saturated cluster). Builds out accent_soft/_dim and gradient stops
    /// procedurally from the accent's HSL coordinates.
    static Theme::Palette fromAccentColor(const QColor &accent,
                                           const Theme::Palette &base);
};

} // namespace SnapClone::UI
