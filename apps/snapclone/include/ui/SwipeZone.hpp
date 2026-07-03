#pragma once
// ─── ui/SwipeZone.hpp ────────────────────────────────────────────────────────
//
// Pure, Qt-free classifier that resolves a horizontal gesture's *intent*:
// is it a PAGE swipe, a SUBMENU (sub-tab) flick, a vertical SCROLL, or a TAP?
//
// The rule, stated plainly (and now testable):
//
//   • PAGE  — the gesture started near a LEFT/RIGHT EDGE, *or* it travelled a
//             long way horizontally. "Reach in from the side, or make a big
//             swipe, and you change pages."
//   • SUBMENU — the gesture started in the CENTRE band *and* was a short, quick
//             flick. "A little flick in the middle switches tabs within a page."
//   • SCROLL — the gesture was vertically dominant.
//   • TAP    — the gesture barely moved.
//
// This only resolves which target the gesture *addresses*. Whether a PAGE/
// SUBMENU gesture actually commits (vs. rubber-bands back) stays with the
// caller's existing distance/velocity commit thresholds — this classifier does
// not change that, it just decides page-vs-submenu-vs-scroll up front so the
// two navigation levels stop fighting over the same drag.
//
// All thresholds are fractions of the viewport (resolution-independent) or
// pixel/velocity floors, defaulting to values aligned with SwipeNavigator's
// existing constants (commit fraction 0.22, tap floor 16px, horizontal axis
// bias 1.5). No Qt, no allocation.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

enum class SwipeIntent { Tap, VerticalScroll, PageSwipe, SubmenuFlick, Ambiguous };

struct SwipeZoneConfig {
    double edgeFraction   = 0.12;  ///< outer 12% of each side counts as an "edge" start
    double longSwipeFrac  = 0.22;  ///< |dx| >= 22% of viewport width ⇒ a "long" (page) swipe
    double flickVelocity  = 0.5;   ///< px/ms; at/above this a short move is a "flick"
    int    flickMinPx     = 24;    ///< a flick must still travel at least this far
    int    tapMaxPx       = 16;    ///< total travel <= this ⇒ a tap, never a swipe
    double horizontalBias = 1.5;   ///< horizontal wins unless |dy| > bias*|dx| (favours nav over scroll)
};

class SwipeZone {
public:
    SwipeZone() = default;
    explicit SwipeZone(const SwipeZoneConfig &cfg) : m_cfg(cfg) {}

    const SwipeZoneConfig &config() const { return m_cfg; }
    void setConfig(const SwipeZoneConfig &cfg) { m_cfg = cfg; }

    /// startX        — gesture's starting x in widget-local px
    /// viewportW     — page/viewport width in px
    /// dx, dy        — total travel from the start point (px); dx<0 = leftward
    /// peakVxAbs     — peak |horizontal velocity| seen during the gesture (px/ms)
    SwipeIntent classify(int startX, int viewportW,
                         int dx, int dy, double peakVxAbs) const
    {
        const int adx = dx < 0 ? -dx : dx;
        const int ady = dy < 0 ? -dy : dy;

        // 1) Barely moved → a tap, regardless of where it started.
        if (adx <= m_cfg.tapMaxPx && ady <= m_cfg.tapMaxPx)
            return SwipeIntent::Tap;

        // 2) Axis: vertical only if clearly vertical (horizontal is favoured so
        //    that diagonal nav swipes aren't stolen by the scroll view).
        const bool horizontal =
            static_cast<double>(adx) * m_cfg.horizontalBias >= static_cast<double>(ady);
        if (!horizontal)
            return SwipeIntent::VerticalScroll;

        // 3) Horizontal: page vs submenu.
        if (viewportW <= 0)
            return SwipeIntent::PageSwipe;   // degenerate; default to page nav

        const double edgePx       = m_cfg.edgeFraction * viewportW;
        const bool startedAtEdge  = startX <= edgePx
                                  || startX >= (viewportW - edgePx);
        const bool longSwipe      = adx >= static_cast<int>(m_cfg.longSwipeFrac * viewportW);
        const bool fastFlick      = peakVxAbs >= m_cfg.flickVelocity
                                  && adx >= m_cfg.flickMinPx;

        // PAGE: came in from an edge, OR swept a long way.
        if (startedAtEdge || longSwipe)
            return SwipeIntent::PageSwipe;
        // SUBMENU: centre band + short, quick flick.
        if (fastFlick)
            return SwipeIntent::SubmenuFlick;
        // Centre band, short and slow: commits to nothing — caller rubber-bands.
        return SwipeIntent::Ambiguous;
    }

    /// Convenience: sign of horizontal travel. -1 = leftward (→ next page in
    /// SwipeNavigator's offset convention), +1 = rightward, 0 = none.
    static int direction(int dx) { return dx < 0 ? -1 : (dx > 0 ? 1 : 0); }

private:
    SwipeZoneConfig m_cfg;
};

} // namespace SnapClone::UI
