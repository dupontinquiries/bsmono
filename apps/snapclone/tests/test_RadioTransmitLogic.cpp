// Standalone (Qt-free, FFI-free) test for the radio transmit/floor orchestration
// state machine. Build: g++ -std=c++20 -I include tests/test_RadioTransmitLogic.cpp -o /tmp/t && /tmp/t
#include "core/RadioTransmitLogic.hpp"
#include <cstdio>

using namespace SnapClone::Core;

static int failures = 0;
static void check(bool cond, const char* msg) {
    if (!cond) { std::printf("FAIL: %s\n", msg); ++failures; }
}

int main() {
    // 1. want + hold → transmit, with a fresh Assert on the grab.
    {
        RadioTransmitLogic l(500);
        auto d = l.step(/*wants=*/true, /*hold=*/true, 1000);
        check(d.transmit, "1: transmit when want+hold");
        check(d.action == FloorBroadcast::Assert, "1: fresh assert on grab");
        check(l.asserting(), "1: asserting after grab");
    }

    // 2. keepalive cadence: re-assert only once the interval elapses.
    {
        RadioTransmitLogic l(500);
        l.step(true, true, 0);                  // grab @ 0
        auto d1 = l.step(true, true, 300);      // 300 < 500 → no keepalive
        check(d1.action == FloorBroadcast::None, "2: no keepalive before interval");
        check(d1.transmit, "2: still transmitting mid-hold");
        auto d2 = l.step(true, true, 500);      // 500 >= 500 → keepalive
        check(d2.action == FloorBroadcast::Assert, "2: keepalive at interval");
        auto d3 = l.step(true, true, 700);      // 200 since last → no keepalive
        check(d3.action == FloorBroadcast::None, "2: no keepalive again until interval");
    }

    // 3. release when the user stops; no double-release afterward.
    {
        RadioTransmitLogic l(500);
        l.step(true, true, 0);
        auto d = l.step(false, false, 100);     // let go
        check(!d.transmit, "3: silent after release");
        check(d.action == FloorBroadcast::Release, "3: release broadcast on stop");
        auto d2 = l.step(false, false, 200);    // already released
        check(d2.action == FloorBroadcast::None, "3: no double release");
        check(!l.asserting(), "3: not asserting after release");
    }

    // 4. want but no hold (a peer holds the floor) → silent, broadcast nothing.
    {
        RadioTransmitLogic l(500);
        auto d = l.step(true, false, 0);
        check(!d.transmit, "4: silent when peer holds the floor");
        check(d.action == FloorBroadcast::None, "4: no broadcast when never held");
    }

    // 5. preempted mid-hold: was transmitting, then lose the floor while still
    //    wanting to talk → go silent and release our (now-lost) claim.
    {
        RadioTransmitLogic l(500);
        l.step(true, true, 0);                  // holding + talking
        auto d = l.step(true, false, 50);       // peer won contention
        check(!d.transmit, "5: silent after preemption");
        check(d.action == FloorBroadcast::Release, "5: release after losing floor");
    }

    // 6. reset() reports whether we were asserting (so caller can broadcast a
    //    final Release), and clears state.
    {
        RadioTransmitLogic l(500);
        l.step(true, true, 0);
        check(l.reset() == true, "6: reset reports prior asserting");
        check(!l.transmitting() && !l.asserting(), "6: cleared after reset");
        check(l.reset() == false, "6: second reset reports nothing");
    }

    // 7. tap-to-talk with no hold ever, then later get the floor.
    {
        RadioTransmitLogic l(500);
        l.step(true, false, 0);                 // wanted, peer had it
        l.step(true, false, 100);               // still blocked
        auto d = l.step(true, true, 200);       // floor freed, we grab it
        check(d.transmit, "7: transmit once floor is finally ours");
        check(d.action == FloorBroadcast::Assert, "7: assert on the late grab");
    }

    if (failures == 0) std::printf("ALL RadioTransmitLogic TESTS PASSED\n");
    return failures ? 1 : 0;
}
