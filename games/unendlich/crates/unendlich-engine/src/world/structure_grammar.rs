//! `structure_grammar` — deterministic placement grammar (L-system + turtle).
//!
//! Worker-03 / WORLD-PERF pure core, round 8. An L-system expands a tiny axiom
//! into a string of turtle commands; a turtle walk turns that into grid
//! placements (village lanes, ruin layouts, dungeon corridors). Everything is
//! deterministic from the inputs + a seed, and expansion is length-capped so a
//! doubling rule can't blow memory. Pairs with `structure_template`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Hard cap on expanded length (characters), to bound rule blow-up.
pub const MAX_LEN: usize = 65_536;

/// Expand `axiom` by applying `rules` (char → replacement) `iters` times. A
/// char with no rule maps to itself. Expansion stops once it reaches `MAX_LEN`
/// so a growth rule can't explode.
pub fn expand(axiom: &str, rules: &[(char, &str)], iters: u32) -> String {
    let mut s: String = axiom.chars().take(MAX_LEN).collect();
    for _ in 0..iters {
        let mut next = String::new();
        let mut capped = false;
        for ch in s.chars() {
            let rep: Option<&str> = rules.iter().find(|&&(c, _)| c == ch).map(|&(_, r)| r);
            match rep {
                Some(r) => next.push_str(r),
                None => next.push(ch),
            }
            if next.len() >= MAX_LEN {
                capped = true;
                break;
            }
        }
        if next.len() > MAX_LEN {
            next.truncate(MAX_LEN);
        }
        let grew = next != s;
        s = next;
        if capped || !grew {
            break; // hit the cap, or reached a fixed point
        }
    }
    s
}

/// A grid placement produced by the turtle walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub z: i32,
    pub kind: u8,
}

/// Walk an expanded command string as a turtle on a grid:
/// * `F` / `G` — place a structure at the current cell, then step forward.
/// * `+` — turn right (clockwise).
/// * `-` — turn left (counter-clockwise).
/// * anything else — ignored.
///
/// `kind` is chosen deterministically from `seed` and the placement index.
pub fn to_placements(expanded: &str, seed: u64) -> Vec<Placement> {
    let mut x: i32 = 0;
    let mut z: i32 = 0;
    let mut dir: u8 = 0; // 0=N, 1=E, 2=S, 3=W
    let mut out: Vec<Placement> = Vec::new();
    for ch in expanded.chars() {
        match ch {
            'F' | 'G' => {
                let kind = ((hash(seed ^ out.len() as u64) >> 8) % 4) as u8;
                out.push(Placement { x, z, kind });
                let (dx, dz) = match dir {
                    0 => (0, -1),
                    1 => (1, 0),
                    2 => (0, 1),
                    _ => (-1, 0),
                };
                x += dx;
                z += dz;
            }
            '+' => dir = (dir + 1) % 4,
            '-' => dir = (dir + 3) % 4,
            _ => {}
        }
    }
    out
}

fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_axiom_expands_as_expected() {
        // Classic algae system: A→AB, B→A.
        let rules = [('A', "AB"), ('B', "A")];
        assert_eq!(expand("A", &rules, 1), "AB");
        assert_eq!(expand("A", &rules, 2), "ABA");
        assert_eq!(expand("A", &rules, 3), "ABAAB");
        assert_eq!(expand("A", &rules, 4), "ABAABABA");
    }

    #[test]
    fn zero_iters_is_axiom() {
        assert_eq!(expand("F+F-G", &[('F', "FF")], 0), "F+F-G");
    }

    #[test]
    fn empty_rules_is_axiom() {
        assert_eq!(expand("F+F+F+F", &[], 5), "F+F+F+F");
    }

    #[test]
    fn length_cap_respected() {
        // A→AA doubles each step; 40 iters would be 2^40 chars uncapped.
        let s = expand("A", &[('A', "AA")], 40);
        assert!(s.len() <= MAX_LEN, "expansion exceeded cap: {}", s.len());
    }

    #[test]
    fn placements_deterministic_from_seed() {
        let s = expand("F", &[('F', "F+F-F")], 3);
        let a = to_placements(&s, 0xABCD);
        let b = to_placements(&s, 0xABCD);
        assert_eq!(a, b);
        // a different seed only changes kinds, not positions.
        let c = to_placements(&s, 0x1234);
        assert_eq!(a.len(), c.len());
        assert!(a.iter().zip(&c).all(|(p, q)| p.x == q.x && p.z == q.z));
    }

    #[test]
    fn turtle_walks_a_square() {
        // F+F+F+F with right turns → unit square back to origin.
        let p = to_placements("F+F+F+F", 0);
        assert_eq!(p.len(), 4);
        assert_eq!((p[0].x, p[0].z), (0, 0)); // start, facing N
        assert_eq!((p[1].x, p[1].z), (0, -1)); // moved N, now facing E
        assert_eq!((p[2].x, p[2].z), (1, -1)); // moved E, now facing S
        assert_eq!((p[3].x, p[3].z), (1, 0)); // moved S, now facing W
    }

    #[test]
    fn expand_deterministic() {
        let rules = [('A', "AB"), ('B', "A")];
        assert_eq!(expand("A", &rules, 6), expand("A", &rules, 6));
    }

    #[test]
    fn kinds_within_range() {
        let p = to_placements("FFFFFFFF", 99);
        assert!(p.iter().all(|q| q.kind < 4));
    }
}
