//! `sim::forage_target` — pick which food site a mob should head for.
//!
//! The selection half of foraging: `forage_memory` *stores* seen food sites,
//! this *chooses* one. Given the mob's hunger, position, and a slice of
//! candidate `(pos, value)` sites (plus an optional per-candidate memory bias),
//! [`select`] returns the index of the best site, or `None` if nothing is worth
//! the trip.
//!
//! Score is `value + memory_bias − distance_cost`, where the distance cost is
//! weighted *up* by hunger: a starving mob grabs the nearest adequate food,
//! while a sated mob can afford to walk to a richer site. Only net-positive
//! sites are eligible, so a site that costs more to reach than it's worth is
//! never chosen. Pure, deterministic, no RNG (R1/R6); standalone
//! `rustc --test`-able, no `unwrap`/`expect` outside tests.

/// A candidate food site: world position `(x, z)` and its food value.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FoodCandidate {
    pub pos: (f32, f32),
    pub value: f32,
}

/// Base distance penalty per unit at zero hunger.
const DIST_BASE: f32 = 0.5;
/// How much hunger amplifies the distance penalty (hungrier ⇒ prefers nearer).
const HUNGER_DIST_GAIN: f32 = 4.0;

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dz = a.1 - b.1;
    (dx * dx + dz * dz).sqrt()
}

/// Score of a single candidate (value + bias − hunger-weighted distance cost).
fn score(hunger: f32, mob_pos: (f32, f32), c: FoodCandidate, bias: f32) -> f32 {
    let h = hunger.max(0.0).min(1.0);
    let weight = DIST_BASE * (1.0 + HUNGER_DIST_GAIN * h);
    c.value + bias - weight * dist(mob_pos, c.pos)
}

/// Choose the best food site for a mob at `mob_pos` with `hunger` (`0..1`).
/// `memory_bias`, if given, adds a per-candidate bonus (e.g. remembered sites);
/// missing/short entries contribute `0`. Returns the index of the highest
/// net-positive score, ties broken toward the lowest index, or `None` if no
/// candidate scores above zero.
pub fn select(
    hunger: f32,
    mob_pos: (f32, f32),
    candidates: &[FoodCandidate],
    memory_bias: Option<&[f32]>,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &c) in candidates.iter().enumerate() {
        let bias = match memory_bias {
            Some(m) => *m.get(i).unwrap_or(&0.0),
            None => 0.0,
        };
        let s = score(hunger, mob_pos, c, bias);
        if !s.is_finite() || s <= 0.0 {
            continue; // net-negative / break-even sites are never chosen
        }
        match best {
            // strict `>` keeps the first (lowest-index) of equal scores → stable
            Some((_, bs)) if s > bs => best = Some((i, s)),
            None => best = Some((i, s)),
            _ => {}
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: f32, z: f32, v: f32) -> FoodCandidate {
        FoodCandidate { pos: (x, z), value: v }
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(select(0.5, (0.0, 0.0), &[], None), None);
    }

    #[test]
    fn high_hunger_picks_nearest_good_low_hunger_picks_richer_far() {
        // near: value 10 @ dist 2 ; far: value 14 @ dist 6
        let sites = [c(2.0, 0.0, 10.0), c(6.0, 0.0, 14.0)];
        // sated → can afford the richer, farther site
        assert_eq!(select(0.0, (0.0, 0.0), &sites, None), Some(1));
        // starving → grabs the nearest adequate site
        assert_eq!(select(1.0, (0.0, 0.0), &sites, None), Some(0));
    }

    #[test]
    fn identical_inputs_same_choice() {
        let sites = [c(3.0, 1.0, 9.0), c(1.0, 1.0, 6.0), c(8.0, 2.0, 20.0)];
        let a = select(0.4, (0.0, 0.0), &sites, None);
        let b = select(0.4, (0.0, 0.0), &sites, None);
        assert_eq!(a, b);
    }

    #[test]
    fn net_negative_never_chosen() {
        // one good site and one whose value can't cover the distance cost
        let sites = [c(1.0, 0.0, 8.0), c(40.0, 0.0, 1.0)];
        assert_eq!(select(0.5, (0.0, 0.0), &sites, None), Some(0));
        // all sites net-negative ⇒ None
        let bad = [c(50.0, 0.0, 0.5), c(0.0, 60.0, 0.2)];
        assert_eq!(select(0.5, (0.0, 0.0), &bad, None), None);
    }

    #[test]
    fn stable_tie_break_lowest_index() {
        // two identical sites ⇒ the first wins
        let sites = [c(2.0, 2.0, 10.0), c(2.0, 2.0, 10.0)];
        assert_eq!(select(0.3, (0.0, 0.0), &sites, None), Some(0));
    }

    #[test]
    fn memory_bias_breaks_a_tie_and_is_optional() {
        let sites = [c(2.0, 2.0, 10.0), c(2.0, 2.0, 10.0)];
        // bias the second (remembered) site above the first
        let bias = [0.0f32, 1.0];
        assert_eq!(select(0.3, (0.0, 0.0), &sites, Some(&bias)), Some(1));
        // no bias ⇒ falls back to the stable tie-break
        assert_eq!(select(0.3, (0.0, 0.0), &sites, None), Some(0));
        // short bias slice doesn't panic; missing entries are 0
        assert_eq!(select(0.3, (0.0, 0.0), &sites, Some(&[0.5])), Some(0));
    }

    #[test]
    fn finite_and_deterministic_over_many() {
        let mut s = 0x31FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..500 {
            let sites: Vec<FoodCandidate> = (0..8)
                .map(|_| c(rng() * 40.0 - 20.0, rng() * 40.0 - 20.0, rng() * 30.0))
                .collect();
            let hunger = rng();
            let pos = (rng() * 10.0, rng() * 10.0);
            let r1 = select(hunger, pos, &sites, None);
            let r2 = select(hunger, pos, &sites, None);
            assert_eq!(r1, r2, "non-deterministic");
            if let Some(i) = r1 {
                assert!(i < sites.len(), "index out of range");
            }
        }
    }
}
