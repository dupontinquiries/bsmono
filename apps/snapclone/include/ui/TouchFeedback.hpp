#pragma once
// ─── TouchFeedback.hpp ────────────────────────────────────────────────────────
//
// Suggestive touch animations — ripple, pulse, spring, and shimmer effects
// that give haptic-like visual feedback on press.
//
// All effects are purely additive (drawn on top, never occlude content),
// GPU-friendly (opacity/scale animations only), and clean up after themselves.
//
// Usage:
//   // On button press:
//   TouchFeedback::ripple(btn);          // expanding circle from press point
//   TouchFeedback::pulse(btn);           // brief scale 1.0 → 1.04 → 1.0
//   TouchFeedback::spring(btn);          // squish → release (playful)
//   TouchFeedback::shimmer(btn);         // left-to-right shine sweep
//
//   // Install persistent press-response on any widget:
//   TouchFeedback::install(btn, TouchFeedback::Effect::Ripple);

#include <QWidget>
#include <QPoint>

namespace SnapClone::UI {

class TouchFeedback
{
public:
    enum class Effect { Ripple, Pulse, Spring, Shimmer };

    /// One-shot ripple from a specific point (use press position).
    static void ripple(QWidget *w, const QPoint &pressPos = QPoint{});

    /// Brief scale pulse — good for confirmation taps.
    static void pulse(QWidget *w);

    /// Spring squish — good for playful actions (sending a snap).
    static void spring(QWidget *w);

    /// Horizontal shimmer — good for loading/sending states.
    static void shimmer(QWidget *w);

    /// Install a persistent effect that fires on every press event.
    /// The widget keeps its normal behaviour; the effect is purely additive.
    static void install(QWidget *w, Effect effect = Effect::Ripple);

private:
    TouchFeedback() = delete;
};

} // namespace SnapClone::UI
