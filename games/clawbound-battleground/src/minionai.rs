//! minionai -- what a lane minion fights, and when it changes its mind.
//!
//! Before this, a minion re-picked "the nearest enemy god in ATTACK range,
//! else the nearest minion" every tick and stood still whenever a god was
//! within 5.5 u. So a cat poking a melee minion from 4 u was never answered
//! -- the minion stopped walking and could not reach -- and a minion never
//! stepped out of its line to reach anything.
//!
//! Now a minion holds a target ([`Foe`]), walks to it, and re-thinks every
//! [`THINK_EVERY`] seconds (or at once when the target dies or gets away)
//! with the usual lane rules, in priority order:
//!
//! 1. **A call for help** ([`Class::Call`]): an enemy god that hurt an
//!    allied god nearby in the last [`CALL_FOR`] seconds. Taken at once,
//!    over anything -- dive a cat under its minions and they turn on you.
//! 2. **Enemy minions** -- the wave fights the wave.
//! 3. **Structures** -- towers and an exposed titan. Above gods: a wave
//!    that reached a tower keeps hitting it while defenders poke at it
//!    (unless they hit one of its cats -- that is a call). With gods
//!    first, pushes stalled at the titan: in AI-only matches 5 of 10 hit
//!    a 14-minute cap with both titans exposed.
//! 4. **Enemy gods** -- when there is nothing else to fight.
//!
//! Within a class it keeps its target unless another is clearly closer
//! (3 u), so a wave does not shuffle between two targets. It follows a
//! target at most [`LEASH`] from where it left its lane, then walks back.
//!
//! Minion targets are held by a stable id, not a `Vec` index: minions are
//! removed as they die, which shifts every later index (see
//! `Game::target_pos_opt`).

use crate::consts::Team;

/// A minion's target. `Minion` holds the target's id (`Minion::id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foe {
    God(usize),
    Minion(u32),
    Tower(usize),
    Titan(Team),
}

/// Priority classes, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Call,
    Minion,
    Structure,
    God,
}

/// A candidate the minion can see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub foe: Foe,
    pub class: Class,
    pub dist: f32,
}

/// Seconds between re-thinks.
pub const THINK_EVERY: f32 = 0.35;
/// How far a minion notices things (it walks to them).
/// TODO: this is too simplistic now because minions will have to intersect their search range and weight with territorial bounds
///       an anger score against the other clan based on towers destroyed and cats attacked should also dynamically affect this
///       throughout a match
pub const AGGRO_MELEE: f32 = 7.0;
pub const AGGRO_CASTER: f32 = 10.0;
/// How far from where it left the lane a minion follows a target.
pub const LEASH: f32 = 9.0;
/// How long a hit on an allied god calls the minions.
pub const CALL_FOR: f32 = 2.0;
/// A same-class target must be this much closer to take over.
pub const SWITCH_MARGIN: f32 = 3.0;

/// The wave's make-up: (melee, casters). Four a wave -- three melee and a
/// caster -- and every third wave a second caster, five.
pub fn wave_for(n: u32) -> (usize, usize) {
    if n % 3 == 2 {
        (3, 2)
    } else {
        (3, 1)
    }
}

/// The target to hold after a re-think. `current` is the held target as
/// seen now (`None` when there is none or it is no longer valid).
pub fn pick(current: Option<Seen>, seen: &[Seen]) -> Option<Foe> {
    let best = seen.iter().min_by(|a, b| a.class.cmp(&b.class).then(a.dist.total_cmp(&b.dist)));
    let (Some(cur), Some(best)) = (current, best) else {
        return best.map(|s| s.foe).or(current.map(|c| c.foe));
    };
    if best.class < cur.class {
        return Some(best.foe);
    }
    if best.class == cur.class && best.foe != cur.foe && best.dist + SWITCH_MARGIN < cur.dist {
        return Some(best.foe);
    }
    Some(cur.foe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(foe: Foe, class: Class, dist: f32) -> Seen {
        Seen { foe, class, dist }
    }

    #[test]
    fn waves_are_four_and_every_third_is_five() {
        let sizes: Vec<usize> = (0..6).map(|n| { let (m, c) = wave_for(n); m + c }).collect();
        assert_eq!(sizes, vec![4, 4, 5, 4, 4, 5]);
    }

    #[test]
    fn minions_fight_minions_before_gods() {
        let seen = [s(Foe::God(3), Class::God, 2.0), s(Foe::Minion(9), Class::Minion, 6.0), s(Foe::Tower(1), Class::Structure, 5.0)];
        assert_eq!(pick(None, &seen), Some(Foe::Minion(9)));
        assert_eq!(pick(None, &seen[..1]), Some(Foe::God(3)), "a god when there is nothing else");
        assert_eq!(pick(None, &[seen[0], seen[2]]), Some(Foe::Tower(1)), "the tower over a defender poking at the wave");
        assert_eq!(pick(None, &[]), None);
    }

    /// The retarget the old minions never did.
    #[test]
    fn a_call_for_help_retargets_at_once() {
        let cur = s(Foe::Minion(4), Class::Minion, 1.5);
        let call = s(Foe::God(2), Class::Call, 6.5);
        assert_eq!(pick(Some(cur), &[cur, call]), Some(Foe::God(2)));
        // and a god being fought gives way to a wave arriving
        let god = s(Foe::God(2), Class::God, 2.0);
        let m = s(Foe::Minion(7), Class::Minion, 5.0);
        assert_eq!(pick(Some(god), &[god, m]), Some(Foe::Minion(7)));
    }

    #[test]
    fn a_target_is_kept_unless_another_is_clearly_closer() {
        let cur = s(Foe::Minion(1), Class::Minion, 4.0);
        let near = s(Foe::Minion(2), Class::Minion, 2.0);
        assert_eq!(pick(Some(cur), &[cur, near]), Some(Foe::Minion(1)), "2 u closer is not enough");
        let nearer = s(Foe::Minion(2), Class::Minion, 0.5);
        assert_eq!(pick(Some(cur), &[cur, nearer]), Some(Foe::Minion(2)));
        // a lower class never displaces a higher one
        let god = s(Foe::God(0), Class::Structure, 0.5);
        assert_eq!(pick(Some(cur), &[cur, god]), Some(Foe::Minion(1)));
        // a call does not displace a call
        let c1 = s(Foe::God(0), Class::Call, 5.0);
        let c2 = s(Foe::God(1), Class::Call, 4.0);
        assert_eq!(pick(Some(c1), &[c1, c2]), Some(Foe::God(0)));
    }
}
