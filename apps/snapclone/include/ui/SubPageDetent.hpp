// ─── ui/SubPageDetent.hpp ─────────────────────────────────────────────────────
//
// SubPageDetent — the resistance + progress curve for the vertical "swipe up/
// down to move between sub-pages (sub-tabs)" gesture.
//
// SwipeNavigator owns the gesture machinery (3400+ lines); this is the small,
// PURE, Qt-free math it should consult so the *feel* is tunable and testable in
// isolation. It maps the raw finger overscroll past a page boundary to:
//
//   • offset(raw)      — how far the content visually slides (rubber-band),
//   • progress(raw)     — [0,1] proximity to the commit threshold, for the
//                         "grow text" submenu animation,
//   • shouldCommit(raw) — whether the sub-page advance fires,
//   • textScale(raw)    — grow-text scale tied to progress (gesture → visual).
//
// The feel encodes three asks, each pinned by a test:
//
//   1. "Detent more to the current page first (more buffer room)."
//        A hard `bufferPx` deadzone — the first few px of overscroll move
//        nothing — PLUS the curve's zero slope at t=0, so leaving the current
//        page takes a deliberate push.
//
//   2. "Get less and less sensitive / more sticky as the gesture elongates and
//      gets closer to crossing the threshold."
//        The displayed-offset curve has DECREASING slope toward commit (zero
//        slope at t=1), so each additional finger-px buys less travel the
//        closer you are — it feels heavier/stickier right before it fires.
//        Net sensitivity is LOW–HIGH–LOW (detent → free middle → sticky commit);
//        the `stickiness` exponent shifts the peak later for a longer sticky
//        tail.
//
//   3. "Less scroll bounce."
//        Travel is capped at a modest `maxOffsetPx` (not a fraction of the
//        page), so there is little overshoot to spring back from. Pair with a
//        NON-overshooting release (see MotionSmooth) instead of a springy one.
//
// No Qt, no allocation, no globals — just the curve. Unit-tested in
// tests/test_SubPageDetent.cpp (compiled and run on CI, no display needed).

#pragma once

#include <cmath>

namespace SnapClone::UI {

struct SubPageDetentConfig {
    /// Hard detent before ANY sub-page motion — pure "buffer room" at the start.
    /// V32.49+: widened 26→64 so reaching the very top/bottom of a scrollable
    /// page (and modest fling overshoot at the edge) stays in the deadzone and
    /// does NOT begin a sub-tab change — you have to deliberately keep pulling.
    double bufferPx = 64.0;
    /// Raw overscroll (px) at which the sub-page advance commits. Larger = more
    /// buffer / harder to cross. (Maps onto SwipeNavigator::kVDetentRawThreshold,
    /// V32.49+: raised 240→340 so the commit is a clearly intentional pull, not
    /// something a flick into the edge trips by accident.)
    double commitPx = 340.0;
    /// Max visible content displacement (px). Caps bounce/travel — keep modest.
    double maxOffsetPx = 64.0;
    /// ≥ 1. Higher ⇒ more initial buffer and a longer, stickier tail near
    /// commit (shifts the peak sensitivity later in the gesture).
    double stickiness = 1.6;
    /// Grow-text scale at progress 0 (gesture just begun) …
    double minTextScale = 0.82;
    /// … and at progress 1 (about to commit). Text grows across this range.
    double maxTextScale = 1.18;
};

class SubPageDetent {
public:
    explicit SubPageDetent(SubPageDetentConfig cfg = {}) : c_(cfg) {}

    /// Visible content displacement for a given raw finger overscroll (px ≥ 0).
    /// Zero within the buffer; rises with decreasing slope toward `maxOffsetPx`.
    double offset(double rawPx) const {
        if (rawPx <= c_.bufferPx) return 0.0;
        const double tw = std::pow(norm(rawPx), c_.stickiness);
        return c_.maxOffsetPx * smootherstep(tw);
    }

    /// [0,1] proximity to the commit threshold — linear in raw past the buffer,
    /// so it is an honest "how close am I to firing" signal for the grow-text.
    double progress(double rawPx) const { return norm(rawPx); }

    /// Has the gesture crossed the commit threshold?
    bool shouldCommit(double rawPx) const { return rawPx >= c_.commitPx; }

    /// Marginal sensitivity d(offset)/d(raw) at `rawPx` (px of travel per px of
    /// finger). Exposed for tuning and for the tests that assert the
    /// low–high–low shape. Numeric central difference — exact enough and
    /// obviously correct.
    double sensitivity(double rawPx) const {
        const double h = 0.5;
        const double lo = rawPx > h ? rawPx - h : 0.0;
        return (offset(rawPx + h) - offset(lo)) / (rawPx + h - lo);
    }

    /// Grow-text scale tied to gesture progress (ties the gesture to a visual
    /// signal). 1.0 is the resting size; below resting early, above near commit.
    double textScale(double rawPx) const { return scaleFromProgress(progress(rawPx)); }

    double scaleFromProgress(double p) const {
        const double t = clamp01(p);
        return c_.minTextScale + (c_.maxTextScale - c_.minTextScale) * t;
    }

    // ── Asymptotic overscroll "crawl" (distinct from the commit curve above) ──
    // For the VISIBLE rubber-band offset. The content keeps following the finger
    // with ever-diminishing motion, asymptotically approaching `ceilingPx` but
    // never reaching it — so it never freezes away from the finger the way a
    // hard clamp does. There is always a little "ceiling space" left to keep
    // nudging the UI, which is what makes a long pull feel attached rather than
    // stuck.
    //
    //   offset = ceilingPx · past / (past + softnessPx)        (past ≥ 0)
    //
    // `softnessPx` sets the initial responsiveness: the slope at past=0 is
    // ceilingPx/softnessPx, so softness ≈ ceiling tracks the finger ~1:1 at
    // first, then eases into the ceiling. The inverse is closed-form, so a grab
    // DURING spring-back can resume from exactly the current visible offset
    // (keeping forward and inverse consistent — a mismatch makes the content
    // jump, which is its own kind of "feels strange").
    static double crawlOffset(double pastDeadzonePx, double ceilingPx, double softnessPx) {
        if (ceilingPx <= 0.0 || softnessPx <= 0.0) return 0.0;
        const double p = pastDeadzonePx > 0.0 ? pastDeadzonePx : 0.0;
        return ceilingPx * p / (p + softnessPx);
    }
    static double crawlInverse(double offsetPx, double ceilingPx, double softnessPx) {
        if (ceilingPx <= 0.0 || softnessPx <= 0.0) return 0.0;
        double o = offsetPx < 0.0 ? 0.0 : offsetPx;
        if (o >= ceilingPx) o = ceilingPx * 0.999999;   // never divide by zero at the asymptote
        return softnessPx * o / (ceilingPx - o);
    }

    const SubPageDetentConfig &config() const { return c_; }

private:
    SubPageDetentConfig c_;

    static double clamp01(double x) { return x < 0.0 ? 0.0 : (x > 1.0 ? 1.0 : x); }

    /// Normalized travel in [0,1] over the (buffer, commit] window.
    double norm(double rawPx) const {
        if (c_.commitPx <= c_.bufferPx) return 0.0;
        return clamp01((rawPx - c_.bufferPx) / (c_.commitPx - c_.bufferPx));
    }

    /// Ken Perlin's smootherstep: zero 1st AND 2nd derivative at both ends, so
    /// motion eases in from the detent and eases out into the sticky commit.
    static double smootherstep(double t) {
        return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    }
};

} // namespace SnapClone::UI
