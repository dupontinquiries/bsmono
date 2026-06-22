// ─── tests/test_SwipeMotion.cpp ──────────────────────────────────────────────
//
// Pins the swipe page-vs-submenu rule and the frame-rate-independent page
// smoothing. Pure C++, no Qt — proves the gesture/zone definition and the lerp
// behaviour without a device or a screen.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/SwipeZone.hpp"
#include "ui/MotionSmooth.hpp"

#include <cstdio>

using namespace SnapClone::UI;

static int g_pass = 0, g_fail = 0;

static const char *intentName(SwipeIntent i)
{
    switch (i) {
        case SwipeIntent::Tap:            return "Tap";
        case SwipeIntent::VerticalScroll: return "VerticalScroll";
        case SwipeIntent::PageSwipe:      return "PageSwipe";
        case SwipeIntent::SubmenuFlick:   return "SubmenuFlick";
        case SwipeIntent::Ambiguous:      return "Ambiguous";
    }
    return "?";
}

static void ck(const char *name, SwipeIntent got, SwipeIntent want)
{
    if (got == want) { ++g_pass; std::printf("  PASS  %-46s -> %s\n", name, intentName(got)); }
    else { ++g_fail; std::printf("  FAIL  %-46s got %s want %s\n", name,
                                  intentName(got), intentName(want)); }
}
static void ckb(const char *name, bool cond)
{
    if (cond) { ++g_pass; std::printf("  PASS  %-46s\n", name); }
    else { ++g_fail; std::printf("  FAIL  %-46s\n", name); }
}

int main()
{
    // 1080px-wide phone viewport for the zone tests.
    const int W = 1080;
    SwipeZone z;  // default config: edge 12%, long 22% (≈238px), flick 0.5px/ms & 24px

    std::printf("\nSwipeZone — page (edges / long) vs submenu (centre / short flick)\n");
    std::printf("-----------------------------------------------------------------\n");
    // edge start, even a modest horizontal move → PAGE
    ck("left-edge start, 120px move",   z.classify(20,   W, -120, 10, 0.3), SwipeIntent::PageSwipe);
    ck("right-edge start, 120px move",  z.classify(1060, W,  120, 10, 0.3), SwipeIntent::PageSwipe);
    // centre start, long horizontal sweep → PAGE (long wins regardless of zone)
    ck("centre start, 300px long sweep",z.classify(540,  W, -300, 20, 0.3), SwipeIntent::PageSwipe);
    // centre start, short quick flick → SUBMENU
    ck("centre start, 60px fast flick", z.classify(540,  W,  -60, 10, 0.9), SwipeIntent::SubmenuFlick);
    // centre start, short and slow → nothing (rubber-bands back)
    ck("centre start, 60px slow drag",  z.classify(540,  W,  -60, 10, 0.1), SwipeIntent::Ambiguous);
    // vertical dominant → SCROLL
    ck("vertical drag",                 z.classify(540,  W,   20, 300, 0.2), SwipeIntent::VerticalScroll);
    // barely moved → TAP
    ck("tiny movement",                 z.classify(540,  W,    8, 6,  0.0), SwipeIntent::Tap);
    // diagonal but horizontal-favoured (|dx|*1.5 >= |dy|) from centre, fast → SUBMENU
    ck("centre diagonal fast flick",    z.classify(540,  W,  -70, 80, 0.9), SwipeIntent::SubmenuFlick);
    // edge start but truly tiny → TAP (tap floor beats edge zone)
    ck("edge start but tiny tap",       z.classify(10,   W,    9, 5,  0.0), SwipeIntent::Tap);

    std::printf("\nMotionSmooth — frame-rate-independent page glide\n");
    std::printf("-----------------------------------------------------------------\n");
    const double hl = 120.0;  // 120ms half-life
    // after exactly one half-life, cover half the distance
    ckb("one half-life covers ~50%",
        MotionSmooth::reached(MotionSmooth::step(0.0, 100.0, hl, hl), 50.0, 0.01));
    // frame-rate independence: two 8ms steps ≈ one 16ms step
    {
        const double oneBig = MotionSmooth::step(0.0, 100.0, 16.0, hl);
        const double half1  = MotionSmooth::step(0.0, 100.0, 8.0, hl);
        const double half2  = MotionSmooth::step(half1, 100.0, 8.0, hl);
        ckb("2x8ms == 1x16ms (rate independent)", MotionSmooth::reached(half2, oneBig, 0.001));
    }
    // a long frame hitch covers more ground but never overshoots
    {
        const double n = MotionSmooth::step(0.0, 100.0, 1000.0, hl);
        ckb("1000ms hitch advances toward target", n > 90.0);
        ckb("never overshoots target",             n <= 100.0);
    }
    // snapToward terminates cleanly when within epsilon
    ckb("snapToward snaps to target when close",
        MotionSmooth::snapToward(99.8, 100.0, 16.0, hl, 0.5) == 100.0);
    // monotonic approach, no wobble, over a simulated 300ms at 16ms frames
    {
        double pos = 0.0; bool monotonic = true; double prev = -1.0;
        for (int t = 0; t < 300; t += 16) {
            pos = MotionSmooth::step(pos, 100.0, 16.0, hl);
            if (pos < prev) monotonic = false;
            prev = pos;
        }
        ckb("monotonic glide, no overshoot wobble", monotonic && pos <= 100.0 && pos > 80.0);
    }

    std::printf("\n-----------------------------------------------------------------\n");
    std::printf("SwipeZone + MotionSmooth: %d passed, %d failed\n\n", g_pass, g_fail);
    return g_fail == 0 ? 0 : 1;
}
