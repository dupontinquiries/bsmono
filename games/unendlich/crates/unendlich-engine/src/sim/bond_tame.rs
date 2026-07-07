//! `sim::bond_tame` — the bond quest that gates a creature's first taming.
//!
//! Taming isn't a single action; it's a slow climb in *trust* driven by how the
//! player treats the creature. This core is the pure step: given the current
//! [`Bond`] stage, an [`Event`], and the current `trust` (`0..1`), [`advance`]
//! returns the next stage and the updated trust.
//!
//! Trust is the real state; the [`Bond`] stage is the band trust currently sits
//! in (`Wild` < `Wary` < `Bonding` < `Tamed`). Kind acts raise trust, harm
//! drops it sharply, and time lets it ebb. The stage only ever moves **one step
//! at a time** toward the band trust now implies, so taming takes a sustained
//! relationship rather than a single lucky feed — and a betrayal knocks a tamed
//! creature back out. Reaching [`Bond::Tamed`] requires crossing the trust
//! threshold. Pure and deterministic: no clock, no randomness.

/// Trust at or above which a creature can be `Tamed`.
pub const TAME_THRESHOLD: f32 = 0.80;
const WARY_AT: f32 = 0.25;
const BONDING_AT: f32 = 0.50;

/// How tame a creature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bond {
    /// Untrusting; flees or ignores.
    Wild,
    /// Tolerates presence.
    Wary,
    /// Actively warming up.
    Bonding,
    /// Tamed — the quest is complete.
    Tamed,
}

/// Something the player (or world) did to the creature this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Offered food — solid trust gain.
    Feed,
    /// Gave a prized gift — the strongest trust gain.
    Gift,
    /// Came near calmly — a small trust gain.
    Approach,
    /// Hurt it — a sharp trust loss.
    Harm,
    /// Simply time passing — a slight decay.
    Time,
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// The trust change for an event.
fn delta(e: Event) -> f32 {
    match e {
        Event::Feed => 0.15,
        Event::Gift => 0.25,
        Event::Approach => 0.05,
        Event::Harm => -0.40,
        Event::Time => -0.02,
    }
}

/// The stage trust falls into, as an ordinal `0..=3`.
fn band(trust: f32) -> u8 {
    if trust >= TAME_THRESHOLD {
        3
    } else if trust >= BONDING_AT {
        2
    } else if trust >= WARY_AT {
        1
    } else {
        0
    }
}

fn ord(b: Bond) -> u8 {
    match b {
        Bond::Wild => 0,
        Bond::Wary => 1,
        Bond::Bonding => 2,
        Bond::Tamed => 3,
    }
}

fn from_ord(o: u8) -> Bond {
    match o {
        0 => Bond::Wild,
        1 => Bond::Wary,
        2 => Bond::Bonding,
        _ => Bond::Tamed,
    }
}

/// Apply an event: update trust, then step the stage one notch toward the band
/// the new trust implies.
///
/// Returns `(next_stage, new_trust)`. The stage never jumps more than one notch
/// per call, so `Tamed` is only ever reached by climbing through `Bonding`, and
/// only while trust is at or above [`TAME_THRESHOLD`].
pub fn advance(b: Bond, e: Event, trust: f32) -> (Bond, f32) {
    let new_trust = clamp01(trust + delta(e));
    let target = band(new_trust);
    let cur = ord(b);

    let next = if target > cur {
        cur + 1
    } else if target < cur {
        cur - 1
    } else {
        cur
    };

    (from_ord(next), new_trust)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn feed_and_gift_raise_trust() {
        let (_, t1) = advance(Bond::Wary, Event::Feed, 0.4);
        assert!(t1 > 0.4);
        let (_, t2) = advance(Bond::Wary, Event::Gift, 0.4);
        assert!(t2 > t1, "gift should raise trust more than feed");
    }

    #[test]
    fn harm_drops_trust_and_regresses() {
        let (stage, t) = advance(Bond::Bonding, Event::Harm, 0.60);
        assert!(t < 0.60, "harm should drop trust");
        assert!(ord(stage) < ord(Bond::Bonding), "harm should regress the stage");
    }

    #[test]
    fn harm_knocks_a_tamed_creature_back_out() {
        let (stage, t) = advance(Bond::Tamed, Event::Harm, 0.90);
        assert_ne!(stage, Bond::Tamed, "a betrayed tame should drop out");
        assert!(t < 0.90);
    }

    #[test]
    fn time_decays_trust_slightly() {
        let (_, t) = advance(Bond::Bonding, Event::Time, 0.60);
        assert!(t < 0.60 && t > 0.50, "time should ebb gently, got {}", t);
    }

    #[test]
    fn reaching_tamed_requires_the_threshold() {
        // just below threshold after the event → not yet tamed
        let (stage_lo, t_lo) = advance(Bond::Bonding, Event::Approach, 0.70);
        assert!(t_lo < TAME_THRESHOLD);
        assert_ne!(stage_lo, Bond::Tamed);

        // crossing the threshold from Bonding → Tamed
        let (stage_hi, t_hi) = advance(Bond::Bonding, Event::Gift, 0.70);
        assert!(t_hi >= TAME_THRESHOLD);
        assert_eq!(stage_hi, Bond::Tamed);
    }

    #[test]
    fn full_wild_to_tamed_path() {
        let mut b = Bond::Wild;
        let mut t = 0.0f32;
        // a steady stream of gifts should walk all the way up
        for _ in 0..6 {
            let (nb, nt) = advance(b, Event::Gift, t);
            b = nb;
            t = nt;
        }
        assert_eq!(b, Bond::Tamed, "trust={}", t);
    }

    #[test]
    fn any_tamed_result_is_above_threshold() {
        // exhaustively: whenever advance returns Tamed, trust >= threshold
        let stages = [Bond::Wild, Bond::Wary, Bond::Bonding, Bond::Tamed];
        let events = [
            Event::Feed,
            Event::Gift,
            Event::Approach,
            Event::Harm,
            Event::Time,
        ];
        let mut seed = 0xB04D_u32;
        for _ in 0..5000 {
            let b = stages[(lcg(&mut seed) as usize) % stages.len()];
            let e = events[(lcg(&mut seed) as usize) % events.len()];
            let trust = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let (stage, nt) = advance(b, e, trust);
            assert!(nt >= 0.0 && nt <= 1.0);
            if stage == Bond::Tamed {
                assert!(nt >= TAME_THRESHOLD, "tamed with trust {}", nt);
            }
            // determinism
            assert_eq!(advance(b, e, trust), (stage, nt));
        }
    }
}
