//! combat.rs — the cat's agile move-set, the **flick controls** that summon each move,
//! and the **combos** that chain them.
//!
//! Design intent: fighting like a cat is fast and stylish. A quick *flick* of the aim in
//! the cat's local frame picks a move by its direction — flick forward to **pounce**,
//! back to **hind-kick**, sideways to **roll**, forward with the low modifier to rake the
//! **underbelly swipe**, a tiny sharp flick to **swat**. Land two moves inside the combo
//! window and the pair flows into a named finisher (a dash-pounce, a counter-gut, …).
//!
//! Pure and deterministic so it unit-tests; the app maps mouse flicks onto `resolve_flick`
//! and the sim plays out each move's windup / active / recovery.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    Pounce,           // a committed forward lunge onto a target
    Jump,             // a vertical hop — clear a log, reposition
    Roll,             // a quick evade roll (i-frames) in a direction
    Dodge,            // a short sidestep
    UnderbellySwipe,  // a low raking swipe — beats a high or pouncing attacker
    HindKick,         // a backward rake with the hind legs — knockback + escape
    Swat,             // a fast paw bat — cheap, low commitment
}

pub struct MoveSpec {
    pub name: &'static str,
    pub windup: f32,    // seconds before the move turns active
    pub active: f32,    // seconds the hit / evade window is live
    pub recovery: f32,  // seconds of lag afterwards
    pub evade: bool,    // grants i-frames while active
    pub power: f32,     // strike / knockback strength (0 = pure utility)
    pub lunge: f32,     // forward distance the move carries you
}

pub fn spec(m: Move) -> MoveSpec {
    match m {
        Move::Pounce          => MoveSpec { name: "Pounce",           windup: 0.10, active: 0.26, recovery: 0.22, evade: false, power: 1.2, lunge: 4.5 },
        Move::Jump            => MoveSpec { name: "Jump",             windup: 0.06, active: 0.30, recovery: 0.14, evade: false, power: 0.0, lunge: 1.0 },
        Move::Roll            => MoveSpec { name: "Roll",             windup: 0.04, active: 0.22, recovery: 0.16, evade: true,  power: 0.0, lunge: 3.2 },
        Move::Dodge           => MoveSpec { name: "Dodge",            windup: 0.03, active: 0.16, recovery: 0.12, evade: true,  power: 0.0, lunge: 2.0 },
        Move::UnderbellySwipe => MoveSpec { name: "Underbelly Swipe", windup: 0.08, active: 0.20, recovery: 0.24, evade: true,  power: 1.0, lunge: 0.6 },
        Move::HindKick        => MoveSpec { name: "Hind Kick",        windup: 0.10, active: 0.18, recovery: 0.26, evade: false, power: 1.4, lunge: -1.4 },
        Move::Swat            => MoveSpec { name: "Swat",             windup: 0.03, active: 0.12, recovery: 0.10, evade: false, power: 0.6, lunge: 0.3 },
    }
}

impl Move {
    pub fn name(self) -> &'static str { spec(self).name }
    pub fn total(self) -> f32 { let s = spec(self); s.windup + s.active + s.recovery }
    pub fn is_attack(self) -> bool { spec(self).power > 0.0 }
}

/// Minimum flick magnitude that reads as a committed move rather than steering.
pub const FLICK_MIN: f32 = 0.55;

/// Resolve a flick — a fast aim gesture, given in the cat's **local frame** (`.1` is
/// forward, `.0` is right) as a unit-ish direction plus a magnitude — into a move.
/// `low` is the underbelly modifier (e.g. a held button) that drops the attack low.
pub fn resolve_flick(dir: (f32, f32), magnitude: f32, low: bool) -> Option<Move> {
    if magnitude < FLICK_MIN { return None; }
    let (side, fwd) = (dir.0, dir.1);
    if low {
        // low modifier: rake underneath going forward, else lash out behind
        return Some(if fwd >= -0.2 { Move::UnderbellySwipe } else { Move::HindKick });
    }
    if fwd > 0.5 { Some(Move::Pounce) }
    else if fwd < -0.5 { Some(Move::HindKick) }
    else if side.abs() > 0.5 { Some(Move::Roll) }
    else { Some(Move::Swat) }   // a sharp, near-directionless flick
}

/// The window (seconds) after a move lands in which a second move flows into a combo.
pub const COMBO_WINDOW: f32 = 0.55;

/// A valid two-move chain flows into a named finisher. Returns the combo's name and the
/// move it resolves into (the flashier version). Order matters.
pub fn combo(prev: Move, next: Move) -> Option<(&'static str, Move)> {
    use Move::*;
    Some(match (prev, next) {
        (Roll, Pounce) | (Dodge, Pounce) => ("Ambush Pounce", Pounce),   // evade, then lunge from cover
        (Dodge, UnderbellySwipe) | (Roll, UnderbellySwipe) => ("Counter Gut", UnderbellySwipe), // slip a blow, rake up
        (Pounce, HindKick) => ("Rake-Off", HindKick),                    // land on them, kick free
        (Pounce, Swat) => ("Pin & Bat", Swat),                           // pin, then batter
        (UnderbellySwipe, Pounce) => ("Rising Pounce", Pounce),          // up off your back into a leap
        (Swat, Swat) => ("Flurry", Swat),                                // a blur of paws
        (HindKick, Roll) => ("Kick Away", Roll),                         // boot off and roll clear
        _ => return None,
    })
}

/// A finisher's damage is amplified over the base move.
pub const COMBO_POWER_BONUS: f32 = 1.5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flick_directions_map_to_moves() {
        assert_eq!(resolve_flick((0.0, 1.0), 1.0, false), Some(Move::Pounce));      // forward
        assert_eq!(resolve_flick((0.0, -1.0), 1.0, false), Some(Move::HindKick));   // back
        assert_eq!(resolve_flick((1.0, 0.0), 1.0, false), Some(Move::Roll));        // side
        assert_eq!(resolve_flick((0.1, 0.1), 1.0, false), Some(Move::Swat));        // sharp, directionless
        assert_eq!(resolve_flick((0.0, 1.0), 1.0, true), Some(Move::UnderbellySwipe)); // low + forward
        assert_eq!(resolve_flick((0.0, -1.0), 1.0, true), Some(Move::HindKick));    // low + back
    }

    #[test]
    fn a_gentle_nudge_is_not_a_flick() {
        assert_eq!(resolve_flick((0.0, 1.0), 0.2, false), None);
    }

    #[test]
    fn combos_chain_into_named_finishers() {
        assert_eq!(combo(Move::Roll, Move::Pounce).map(|c| c.0), Some("Ambush Pounce"));
        assert_eq!(combo(Move::Dodge, Move::UnderbellySwipe).map(|c| c.0), Some("Counter Gut"));
        assert_eq!(combo(Move::Swat, Move::Swat).map(|c| c.0), Some("Flurry"));
        // order matters — the reverse of a combo isn't automatically one
        assert!(combo(Move::Pounce, Move::Roll).is_none());
        // an arbitrary pair isn't a combo
        assert!(combo(Move::Jump, Move::Swat).is_none());
    }

    #[test]
    fn attacks_and_utility_are_distinguished() {
        assert!(Move::Pounce.is_attack() && Move::HindKick.is_attack() && Move::UnderbellySwipe.is_attack());
        assert!(!Move::Roll.is_attack() && !Move::Jump.is_attack() && !Move::Dodge.is_attack());
        // evades grant i-frames; lunges carry you; a hind-kick drives you backward
        assert!(spec(Move::Roll).evade && spec(Move::HindKick).lunge < 0.0);
    }
}
