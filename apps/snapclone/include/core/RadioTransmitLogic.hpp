#ifndef SNAPCLONE_CORE_RADIOTRANSMITLOGIC_HPP
#define SNAPCLONE_CORE_RADIOTRANSMITLOGIC_HPP

#include <cstdint>

namespace SnapClone::Core {

// What the session should tell peers about its floor claim this step.
enum class FloorBroadcast {
    None,     // no change — say nothing
    Assert,   // broadcast "I hold the floor" (a fresh grab OR a keepalive)
    Release,  // broadcast "I released the floor"
};

struct RadioTransmitDecision {
    bool transmit = false;                       // encode+send the mic this instant?
    FloorBroadcast action = FloorBroadcast::None; // what to tell peers
};

// Pure, Qt-free orchestration sitting between the Rust transmit gate + floor
// control and the Qt session. Each step it is told whether the local user
// currently INTENDS to talk (`wants`, from sc_radio_gate_update) and whether we
// currently HOLD the floor (`hold`, from sc_group_floor_request / _holds), and
// decides what to actually do: transmit or not, and whether to (re)assert or
// release our floor claim to peers.
//
// Why a separate type: the gate and floor each have their own (well-tested)
// state machines in Rust, but the glue *between* them — "request the stick when
// you want to talk, transmit only while you both want it and hold it, keep
// broadcasting so peers don't time you out, release the moment you stop" — is
// its own small state machine, and keeping it Qt-free makes it directly
// testable without an event loop or the FFI.
//
// Keepalive: while transmitting, re-`Assert` every `reassert_interval_ms` so a
// peer's floor timeout never reaps a holder who is actively talking.
class RadioTransmitLogic {
public:
    explicit RadioTransmitLogic(std::uint64_t reassert_interval_ms = 500)
        : reassert_interval_ms_(reassert_interval_ms) {}

    // `now_ms` must be monotonic non-decreasing (a steady/elapsed clock).
    RadioTransmitDecision step(bool wants, bool hold, std::uint64_t now_ms) {
        const bool transmit = wants && hold;
        FloorBroadcast action = FloorBroadcast::None;

        if (transmit) {
            if (!asserting_) {
                // Fresh grab: we just took the floor and are starting to talk.
                asserting_ = true;
                last_assert_ms_ = now_ms;
                action = FloorBroadcast::Assert;
            } else if (now_ms - last_assert_ms_ >= reassert_interval_ms_) {
                // Still talking: periodic keepalive so peers don't time us out.
                last_assert_ms_ = now_ms;
                action = FloorBroadcast::Assert;
            }
        } else {
            if (asserting_) {
                // We were holding+talking and now stopped (let go, VOX closed,
                // or we were preempted and lost the floor) — tell peers we're
                // done so the stick frees immediately instead of on timeout.
                asserting_ = false;
                action = FloorBroadcast::Release;
            }
            // (wants && !hold): we want to talk but a peer holds the floor — we
            // never asserted, so stay silent and broadcast nothing.
        }

        transmitting_ = transmit;
        return { transmit, action };
    }

    // Force the channel closed (teardown / floor revoked). Returns true if we
    // were asserting, so the caller knows to broadcast a final Release.
    bool reset() {
        const bool was_asserting = asserting_;
        asserting_ = false;
        transmitting_ = false;
        return was_asserting;
    }

    bool transmitting() const { return transmitting_; }
    bool asserting() const { return asserting_; }

private:
    std::uint64_t reassert_interval_ms_;
    std::uint64_t last_assert_ms_ = 0;
    bool asserting_ = false;    // we currently claim the floor (and broadcast it)
    bool transmitting_ = false; // mic is live this step
};

} // namespace SnapClone::Core

#endif // SNAPCLONE_CORE_RADIOTRANSMITLOGIC_HPP
