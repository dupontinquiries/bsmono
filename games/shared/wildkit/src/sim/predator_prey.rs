//! `sim::predator_prey` — the reactive ecology decision: given what a creature
//! can see around it, does it **flee**, **hunt**, or carry on?
//!
//! This is a pure read-only choice — it mutates no world and moves nothing. It
//! just looks at the neighbours a creature already perceives and returns an
//! [`Act`] for the brain to carry out. The rule is a fixed priority:
//!
//! 1. **Flee** the nearest thing that threatens me — survival first, always.
//! 2. else, if I'm hungry, **Hunt** the nearest thing I can eat that isn't
//!    bigger than me.
//! 3. else **Ignore** — nothing worth reacting to.
//!
//! Reactive tier: cheap, allocation-free, deterministic. Ties (equal distance)
//! resolve to the earlier entry in the slice, so the same perception always
//! yields the same act on every peer. Input types are self-contained so the
//! file builds and tests standalone.

/// Hunger at or above which a creature is willing to hunt.
pub const HUNGRY_AT: f32 = 0.5;

/// The deciding creature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Me {
    /// Species/kind tag (opaque here).
    pub kind: u8,
    /// Body size; a creature won't hunt anything larger than itself.
    pub size: f32,
    /// Hunger, `0..1`; `>= HUNGRY_AT` means willing to hunt.
    pub hunger: f32,
}

/// A perceived neighbour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Other {
    /// Stable entity handle, returned inside the chosen [`Act`].
    pub idx: usize,
    /// Species/kind tag.
    pub kind: u8,
    /// Distance from me; smaller is nearer.
    pub dist: f32,
    /// The neighbour's body size.
    pub size: f32,
    /// Whether my diet/abilities let me eat it.
    pub edible_by_me: bool,
    /// Whether it threatens me.
    pub threat_to_me: bool,
}

/// The decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Flee the entity with this `idx`.
    Flee(usize),
    /// Hunt the entity with this `idx`.
    Hunt(usize),
    /// Do nothing reactive.
    Ignore,
}

/// Return the nearer of the current best and a candidate, keeping the existing
/// best on a tie (so earlier slice entries win — deterministic).
fn nearer<'a>(best: Option<&'a Other>, cand: &'a Other) -> Option<&'a Other> {
    match best {
        Some(b) if b.dist <= cand.dist => Some(b),
        _ => Some(cand),
    }
}

/// Decide what to do about the perceived neighbours.
pub fn decide(me: &Me, others: &[Other]) -> Act {
    // 1. Flee the nearest threat — unconditional priority.
    let mut threat: Option<&Other> = None;
    for o in others {
        if o.threat_to_me {
            threat = nearer(threat, o);
        }
    }
    if let Some(t) = threat {
        return Act::Flee(t.idx);
    }

    // 2. Hunt the nearest edible prey that isn't bigger than me, if hungry.
    if me.hunger >= HUNGRY_AT {
        let mut prey: Option<&Other> = None;
        for o in others {
            if o.edible_by_me && o.size <= me.size {
                prey = nearer(prey, o);
            }
        }
        if let Some(p) = prey {
            return Act::Hunt(p.idx);
        }
    }

    // 3. Nothing worth reacting to.
    Act::Ignore
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 100_000.0
    }

    fn other(idx: usize, dist: f32, size: f32, edible: bool, threat: bool) -> Other {
        Other {
            idx,
            kind: 0,
            dist,
            size,
            edible_by_me: edible,
            threat_to_me: threat,
        }
    }

    #[test]
    fn flee_beats_hunt_under_threat() {
        let me = Me {
            kind: 1,
            size: 5.0,
            hunger: 1.0,
        };
        // a close edible prey AND a farther threat: flee still wins
        let others = [
            other(10, 1.0, 1.0, true, false), // tasty, near
            other(20, 5.0, 9.0, false, true), // scary, far
        ];
        assert_eq!(decide(&me, &others), Act::Flee(20));
    }

    #[test]
    fn hunt_only_when_hungry_with_prey() {
        let prey = [other(7, 2.0, 1.0, true, false)];

        let full = Me {
            kind: 1,
            size: 5.0,
            hunger: 0.0,
        };
        assert_eq!(decide(&full, &prey), Act::Ignore);

        let hungry = Me {
            kind: 1,
            size: 5.0,
            hunger: 0.9,
        };
        assert_eq!(decide(&hungry, &prey), Act::Hunt(7));

        // hungry but no prey present
        let no_prey = [other(8, 2.0, 1.0, false, false)];
        assert_eq!(decide(&hungry, &no_prey), Act::Ignore);
    }

    #[test]
    fn nearest_target_chosen() {
        let me = Me {
            kind: 1,
            size: 5.0,
            hunger: 1.0,
        };
        // nearest threat among several
        let threats = [
            other(1, 9.0, 9.0, false, true),
            other(2, 3.0, 9.0, false, true),
            other(3, 6.0, 9.0, false, true),
        ];
        assert_eq!(decide(&me, &threats), Act::Flee(2));

        // nearest prey among several (no threats)
        let prey = [
            other(1, 9.0, 1.0, true, false),
            other(2, 4.0, 1.0, true, false),
            other(3, 7.0, 1.0, true, false),
        ];
        assert_eq!(decide(&me, &prey), Act::Hunt(2));
    }

    #[test]
    fn wont_hunt_anything_bigger() {
        let me = Me {
            kind: 1,
            size: 2.0,
            hunger: 1.0,
        };
        // edible, but bigger than me → not hunted
        let big = [other(5, 1.0, 3.0, true, false)];
        assert_eq!(decide(&me, &big), Act::Ignore);

        // same size is allowed
        let equal = [other(6, 1.0, 2.0, true, false)];
        assert_eq!(decide(&me, &equal), Act::Hunt(6));
    }

    #[test]
    fn empty_neighbours_is_ignore() {
        let me = Me {
            kind: 1,
            size: 5.0,
            hunger: 1.0,
        };
        assert_eq!(decide(&me, &[]), Act::Ignore);
    }

    #[test]
    fn ties_resolve_to_earlier_entry() {
        let me = Me {
            kind: 1,
            size: 5.0,
            hunger: 1.0,
        };
        let prey = [
            other(100, 4.0, 1.0, true, false),
            other(200, 4.0, 1.0, true, false), // equal distance
        ];
        assert_eq!(decide(&me, &prey), Act::Hunt(100));
    }

    #[test]
    fn deterministic_and_threat_always_outranks_prey() {
        let mut seed = 0xEC01_u32;
        for _ in 0..3000 {
            let me = Me {
                kind: (lcg(&mut seed) & 7) as u8,
                size: unit(&mut seed) * 10.0,
                hunger: unit(&mut seed),
            };
            let n = (lcg(&mut seed) % 6) as usize;
            let mut v = Vec::new();
            let mut any_threat = false;
            for i in 0..n {
                let threat = lcg(&mut seed) & 3 == 0;
                any_threat |= threat;
                v.push(other(
                    i,
                    unit(&mut seed) * 10.0,
                    unit(&mut seed) * 10.0,
                    lcg(&mut seed) & 1 == 0,
                    threat,
                ));
            }
            let a = decide(&me, &v);
            assert_eq!(a, decide(&me, &v), "non-deterministic");
            // if any threat exists, the act must be a Flee
            if any_threat {
                assert!(matches!(a, Act::Flee(_)), "threat present but didn't flee");
            }
        }
    }
}
