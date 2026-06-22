// ─── tests/test_PressFsm.cpp ─────────────────────────────────────────────────
//
// Simulation harness for the tap / long-press / drag-cancel decision core.
//
// Two jobs:
//   1. Regression: pin the gesture rules so a refactor can't silently change
//      what a tap, a long-press, or a cancelled drag means.
//   2. UX evidence for first-time phone users. Someone who has never held a
//      smartphone produces *imprecise* input: they press slowly, their finger
//      shakes, they lift off near the edge of a button, they "tap" by holding
//      for most of a second. This harness replays exactly those messy inputs
//      and checks the gesture is still understood — and measures how a more
//      forgiving threshold profile changes the outcome, so the forgiveness
//      knobs are tuned with data instead of by feel.
//
// Pure C++: no Qt, no GUI, no device. Builds and runs anywhere.
//
// The driver replays a millisecond timeline through PressFsm the same way
// PressGesture does on a real phone: a press arms a single-shot long-press
// timer at (press-time + longPressMs); it fires only if it comes due strictly
// before the next real event. A release landing exactly on the deadline is a
// tap — the user beat the timer.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/PressFsm.hpp"

#include <cstdio>
#include <string>
#include <vector>

using namespace SnapClone::UI;

// ── synthetic input event on a ms timeline ───────────────────────────────────
struct Ev {
    int  t;             // timestamp (ms)
    char kind;          // 'P' press · 'M' move · 'R' release · 'C' cancel
    int  x = 0, y = 0;
    bool inside = true; // for 'R': did the finger lift inside the target rect?
};

static const char *outcomeName(PressOutcome o)
{
    switch (o) {
        case PressOutcome::None:         return "None";
        case PressOutcome::Tap:          return "Tap";
        case PressOutcome::LongPress:    return "LongPress";
        case PressOutcome::DragCanceled: return "DragCanceled";
    }
    return "?";
}

struct RunResult {
    std::vector<PressOutcome> outcomes;        // non-None outcomes, in order
    PressState finalState = PressState::Idle;
};

// Replay one timeline. Mirrors PressGesture: only a press that actually enters
// Pressing (from Idle) arms the timer — a re-press while already pressing
// (touch + synthesized mouse for the same finger) is ignored and does NOT
// restart the timer.
static RunResult run(PressFsm &fsm, const std::vector<Ev> &evs)
{
    RunResult r;
    long armed = -1;  // scheduled long-press timeout (ms); -1 = disarmed
    auto emit = [&](PressOutcome o) { if (o != PressOutcome::None) r.outcomes.push_back(o); };

    for (const Ev &e : evs) {
        if (armed >= 0 && armed < e.t) {       // timer comes due before this event
            emit(fsm.onLongPressTimeout());
            armed = -1;
        }
        switch (e.kind) {
            case 'P': {
                const bool wasIdle = fsm.isIdle();
                emit(fsm.onPress(e.x, e.y));
                if (wasIdle && fsm.isPressing())
                    armed = static_cast<long>(e.t) + fsm.longPressMs();
                break;
            }
            case 'M': emit(fsm.onMove(e.x, e.y)); break;
            case 'R': emit(fsm.onRelease(e.inside)); armed = -1; break;
            case 'C': emit(fsm.onCancel());           armed = -1; break;
        }
    }
    if (armed >= 0)                              // finger held past end of input
        emit(fsm.onLongPressTimeout());
    r.finalState = fsm.state();
    return r;
}

// ── tiny assert/report framework ─────────────────────────────────────────────
static int g_pass = 0, g_fail = 0;

static std::string seq(const std::vector<PressOutcome> &v)
{
    std::string s;
    for (auto o : v) { s += outcomeName(o); s += ' '; }
    if (s.empty()) s = "(none) ";
    return s;
}

static void check(const char *scenario, int longPressMs, int moveCancelPx,
                  const std::vector<Ev> &evs,
                  std::vector<PressOutcome> expect,
                  PressState expectState = PressState::Idle)
{
    PressFsm fsm(longPressMs, moveCancelPx);
    RunResult r = run(fsm, evs);
    const bool ok = (r.outcomes == expect) && (r.finalState == expectState);
    if (ok) {
        ++g_pass;
        std::printf("  PASS  %-40s -> [ %s]\n", scenario, seq(r.outcomes).c_str());
    } else {
        ++g_fail;
        std::printf("  FAIL  %-40s\n        got      [ %s] state=%d\n"
                    "        expected [ %s] state=%d\n",
                    scenario, seq(r.outcomes).c_str(), static_cast<int>(r.finalState),
                    seq(expect).c_str(), static_cast<int>(expectState));
    }
}

// default profile: Android-standard 500 ms / 16 px
static void def(const char *scenario, const std::vector<Ev> &evs,
                std::vector<PressOutcome> expect, PressState st = PressState::Idle)
{
    check(scenario, 500, 16, evs, expect, st);
}

static void expectEq(const char *what, int got, int want)
{
    if (got == want) { ++g_pass; std::printf("  PASS  %-40s -> %d\n", what, got); }
    else { ++g_fail; std::printf("  FAIL  %-40s got %d want %d\n", what, got, want); }
}

int main()
{
    using O = PressOutcome;

    std::printf("\nPressFsm — default profile (longPress=500ms, moveCancel=16px)\n");
    std::printf("------------------------------------------------------------\n");

    // ── the everyday cases ────────────────────────────────────────────────────
    def("clean quick tap",
        {{0,'P',100,100}, {120,'R',100,100,true}}, {O::Tap});

    def("deliberate long-press (hold to edit)",
        {{0,'P',100,100}, {900,'R',100,100,true}}, {O::LongPress});

    def("swipe / scroll (big horizontal drag)",
        {{0,'P',100,100}, {40,'M',140,100}, {80,'M',200,100}}, {O::DragCanceled});

    def("two taps in a row re-arm cleanly",
        {{0,'P',100,100}, {100,'R',100,100,true},
         {200,'P',100,100}, {260,'R',100,100,true}}, {O::Tap, O::Tap});

    def("platform cancels the touch mid-press",
        {{0,'P',100,100}, {90,'C',0,0}}, {O::DragCanceled});

    // ── first-time-user imprecision ─────────────────────────────────────────────
    // A novice reacts slowly. As long as they let go before the long-press
    // deadline, a slow tap is still a tap.
    def("hesitant slow tap (460ms, still under 500)",
        {{0,'P',100,100}, {460,'R',100,100,true}}, {O::Tap});

    // Their finger trembles while pressing. Jitter under the move threshold
    // must not cancel the tap.
    def("jittery finger, drift stays < 16px",
        {{0,'P',100,100}, {50,'M',106,103}, {90,'M',110,104}, {200,'R',110,104,true}},
        {O::Tap});

    // Exactly-at-threshold move is NOT a cancel (the test is strictly greater).
    def("drift of exactly 16px (boundary, not cancelled)",
        {{0,'P',100,100}, {50,'M',108,108}, {120,'R',108,108,true}}, {O::Tap});

    // They drift too far while "tapping" — that reads as the start of a drag.
    def("finger wanders 26px → treated as a drag",
        {{0,'P',100,100}, {80,'M',118,108}, {200,'R',118,108,true}}, {O::DragCanceled});

    // Fat-finger release: they lift off just outside the button.
    def("releases just outside the target → not a tap",
        {{0,'P',100,100}, {300,'R',140,140,false}}, {O::DragCanceled});

    // The classic novice trap: they "tap" by pressing and holding for most of
    // a second. Under the default profile this becomes a long-press, not the
    // tap they intended. The harness pins this so we SEE the trap.
    def("over-slow 'tap' (700ms hold) trips long-press",
        {{0,'P',100,100}, {700,'R',100,100,true}}, {O::LongPress});

    // Same finger arriving as touch + synthesized mouse must fire one gesture.
    def("touch + synth-mouse double press → one tap",
        {{0,'P',100,100}, {10,'P',100,100}, {120,'R',100,100,true}}, {O::Tap});

    // ── threshold clamps ────────────────────────────────────────────────────────
    std::printf("\nthreshold clamps\n----------------\n");
    { PressFsm f(10, 1); expectEq("longPressMs clamps up to 50", f.longPressMs(), 50);
                         expectEq("moveCancelPx clamps up to 2", f.moveCancelPx(), 2); }

    // ── forgiveness profile: what a gentler setting buys a first-timer ──────────
    std::printf("\nfirst-timer forgiveness profile (longPress=900ms, moveCancel=28px)\n");
    std::printf("-----------------------------------------------------------------\n");

    // The 700ms hold that became a long-press above is now correctly a tap,
    // because the long-press deadline moved out to 900ms.
    check("over-slow 700ms 'tap' is now a Tap", 900, 28,
          {{0,'P',100,100}, {700,'R',100,100,true}}, {O::Tap});

    // A shakier hand (24px of drift) no longer cancels.
    check("shakier 24px drift is now tolerated", 900, 28,
          {{0,'P',100,100}, {120,'M',114,110}, {300,'R',114,110,true}}, {O::Tap});

    // The deliberate long-press still works — it just takes a beat longer.
    check("deliberate long-press still fires (at 900ms)", 900, 28,
          {{0,'P',100,100}, {1200,'R',100,100,true}}, {O::LongPress});

    // ── summary ─────────────────────────────────────────────────────────────────
    std::printf("\n------------------------------------------------------------\n");
    std::printf("PressFsm harness: %d passed, %d failed\n\n", g_pass, g_fail);
    if (g_fail == 0) {
        std::printf("Forgiveness readout:\n");
        std::printf("  • default: a tap may take up to 500ms before it becomes a\n");
        std::printf("    long-press, and may wander up to 16px before it cancels.\n");
        std::printf("  • a hesitant first-timer who holds ~0.6-0.8s to 'tap' will\n");
        std::printf("    trigger a long-press instead — the over-slow-tap case above.\n");
        std::printf("  • the gentler profile (900ms / 28px) keeps slow, shaky taps\n");
        std::printf("    working at the cost of a slightly slower long-press.\n\n");
    }
    return g_fail == 0 ? 0 : 1;
}
