#pragma once
// ─── ui/MotionSmooth.hpp ─────────────────────────────────────────────────────
//
// Frame-rate-independent smoothing for animated positions — the "pixel lerp"
// that turns an occasional hard page jump into a glide.
//
// A naive lerp (next = current + (target-current)*k) is frame-rate dependent:
// the same k feels different at 60Hz vs 90Hz vs a frame hitch, which is exactly
// what produces a visible "jump" when frames are uneven. The fix is to drive
// the blend off ELAPSED TIME via a half-life: however long the last frame took,
// the position covers a consistent fraction of the remaining distance.
//
//   factor = 0.5 ^ (dt / halfLife)      // dt and halfLife in ms
//   next   = target + (current - target) * factor
//
// After one half-life the position is halfway to target, after two ~75%, etc.,
// regardless of how that time was split into frames. It never overshoots
// (factor ∈ (0,1]) so there's no spring wobble — a calm glide, matching the
// app's muted feel. Use snapToward() to also terminate cleanly when close.
//
// Pure: no Qt, no allocation. Header-only.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

class MotionSmooth {
public:
    /// One exponential-smoothing step toward `target`. dtMs = time since last
    /// step; halfLifeMs = time to cover half the remaining distance (smaller =
    /// snappier). Both clamped to sane floors so a zero/negative dt or halflife
    /// can't divide-by-zero or freeze the animation.
    static double step(double current, double target, double dtMs, double halfLifeMs)
    {
        if (dtMs <= 0.0) return current;
        if (halfLifeMs < 1.0) halfLifeMs = 1.0;
        const double factor = pow2(-dtMs / halfLifeMs);   // 0.5 ^ (dt/halfLife)
        return target + (current - target) * factor;
    }

    /// Like step(), but snaps exactly to target once within `snapEpsilon` so the
    /// animation terminates instead of asymptotically crawling forever. Returns
    /// the next position; check reached() or compare to target to stop a timer.
    static double snapToward(double current, double target, double dtMs,
                             double halfLifeMs, double snapEpsilon = 0.5)
    {
        const double next = step(current, target, dtMs, halfLifeMs);
        return (absd(target - next) <= snapEpsilon) ? target : next;
    }

    /// True once `current` is within `snapEpsilon` of `target`.
    static bool reached(double current, double target, double snapEpsilon = 0.5)
    {
        return absd(target - current) <= snapEpsilon;
    }

private:
    static double absd(double v) { return v < 0.0 ? -v : v; }

    // pow2(e) = 2^e, without pulling in <cmath> at every include site. Uses a
    // simple exp/ln identity via a small series-free path: fall back to a
    // compiler builtin where available.
    static double pow2(double e)
    {
        // 2^e = e^(e * ln2). __builtin_exp is constexpr-friendly on g++/clang.
        constexpr double kLn2 = 0.6931471805599453;
        return __builtin_exp(e * kLn2);
    }
};

} // namespace SnapClone::UI
