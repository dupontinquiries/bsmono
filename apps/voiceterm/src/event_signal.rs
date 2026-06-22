//! v0.1.750: cheap cross-layer event signaling. The QML layer used to poll
//! several backend event queues independently (wake-word, ask-AI, voice
//! result, …), each taking a mutex every tick even when empty — many timers,
//! each "penetrating deep" to the locked layer on every wakeup.
//!
//! This is the cheap top-level signal that gates that deep penetration: one
//! process-global lock-free `AtomicU32` bitmask. Producers `raise(channel)`
//! when they enqueue (a single relaxed atomic OR — effectively free, no lock);
//! the consumer reads the whole word once with `peek()` (one relaxed load) and
//! only locks/drains the specific queue whose bit is set. When nothing is
//! pending the consumer never touches a mutex at all.
//!
//! Invariant: `raise` always happens on enqueue, so an event is NEVER missed.
//! `clear` may race a concurrent enqueue, but the only consequence is one extra
//! (cheap) peek — never a lost event. Pure + tested.

use std::sync::atomic::{AtomicU32, Ordering};

/// Event channels the QML layer polls. Each maps to one bit. Add channels here
/// as more poll loops are migrated onto the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Channel {
    Wake = 1 << 0,
    AskAi = 1 << 1,
    VoiceResult = 1 << 2,
    LlmResult = 1 << 3,
    Confirm = 1 << 4,
}

impl Channel {
    #[inline]
    pub fn bit(self) -> u32 { self as u32 }
}

/// The global pending-events bitmask. Relaxed ordering is sufficient: we only
/// need eventual visibility (the poll runs repeatedly), not synchronization of
/// other memory — the queue's own mutex provides that for the payload.
static PENDING: AtomicU32 = AtomicU32::new(0);

/// Mark a channel as having a pending event (producer side, on enqueue). One
/// atomic OR — no lock, no allocation.
#[inline]
pub fn raise(ch: Channel) {
    PENDING.fetch_or(ch.bit(), Ordering::Relaxed);
}

/// Clear a channel's bit (consumer side, after it has drained the queue).
#[inline]
pub fn clear(ch: Channel) {
    PENDING.fetch_and(!ch.bit(), Ordering::Relaxed);
}

/// The whole pending bitmask — one cheap relaxed load. The consumer reads this
/// once per tick and only descends into a channel whose bit is set.
#[inline]
pub fn peek() -> u32 {
    PENDING.load(Ordering::Relaxed)
}

/// Is a specific channel pending?
#[inline]
pub fn is_set(ch: Channel) -> bool {
    PENDING.load(Ordering::Relaxed) & ch.bit() != 0
}

/// Test-and-clear a channel: returns whether it was set, clearing it. Useful
/// when the consumer will unconditionally drain the queue after.
#[inline]
pub fn take(ch: Channel) -> bool {
    PENDING.fetch_and(!ch.bit(), Ordering::Relaxed) & ch.bit() != 0
}

/// True if nothing at all is pending — the fast path the poll uses to avoid
/// ANY deeper work.
#[inline]
pub fn idle() -> bool {
    PENDING.load(Ordering::Relaxed) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // tests share the global PENDING, so reset at the start of each to stay
    // independent of ordering.
    fn reset() { PENDING.store(0, Ordering::Relaxed); }

    #[test]
    fn raise_sets_only_that_bit() {
        reset();
        assert!(idle());
        raise(Channel::Wake);
        assert!(is_set(Channel::Wake));
        assert!(!is_set(Channel::AskAi));
        assert!(!idle());
        assert_eq!(peek(), Channel::Wake.bit());
    }

    #[test]
    fn multiple_channels_coexist() {
        reset();
        raise(Channel::Wake);
        raise(Channel::LlmResult);
        assert!(is_set(Channel::Wake));
        assert!(is_set(Channel::LlmResult));
        assert_eq!(peek(), Channel::Wake.bit() | Channel::LlmResult.bit());
    }

    #[test]
    fn clear_leaves_others() {
        reset();
        raise(Channel::Wake);
        raise(Channel::AskAi);
        clear(Channel::Wake);
        assert!(!is_set(Channel::Wake));
        assert!(is_set(Channel::AskAi));
    }

    #[test]
    fn take_returns_and_clears() {
        reset();
        raise(Channel::Confirm);
        assert!(take(Channel::Confirm));   // was set
        assert!(!is_set(Channel::Confirm)); // now cleared
        assert!(!take(Channel::Confirm));  // already clear
    }

    #[test]
    fn idle_when_nothing_pending() {
        reset();
        assert!(idle());
        raise(Channel::VoiceResult);
        assert!(!idle());
        clear(Channel::VoiceResult);
        assert!(idle());
    }

    #[test]
    fn raise_is_idempotent() {
        reset();
        raise(Channel::Wake);
        raise(Channel::Wake);
        raise(Channel::Wake);
        assert_eq!(peek(), Channel::Wake.bit()); // still just one bit
    }

    #[test]
    fn distinct_bits_per_channel() {
        // no two channels share a bit (would cause cross-talk).
        let chans = [Channel::Wake, Channel::AskAi, Channel::VoiceResult,
                     Channel::LlmResult, Channel::Confirm];
        for (i, a) in chans.iter().enumerate() {
            for b in &chans[i + 1..] {
                assert_ne!(a.bit(), b.bit(), "channels must have distinct bits");
            }
        }
    }
}
