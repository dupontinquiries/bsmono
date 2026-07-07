//! `sim::hybrid_rules` — which element pairings are allowed to cross, and what
//! they blend into.
//!
//! Elements combine to make hybrids (the storm that is air-meets-water, the
//! charged fire of fire-meets-air). Not every pairing is meaningful, so
//! [`cross`] returns `Option<Hybrid>`: `Some` with the blended [`Element`] and a
//! bounded `potency` for an allowed pairing, `None` for one that doesn't mix.
//!
//! Two guarantees the rest of the game leans on:
//! - **Same element crosses to itself** at full potency (a pure expression).
//! - **Crossing is symmetric**: `cross(a, b) == cross(b, a)`. This is enforced
//!   structurally — the pair is ordered by a fixed element index before the
//!   table lookup, so there is only ever one entry to keep in sync.
//!
//! Pure and table-driven: no clock, no allocation, fully deterministic.

/// The elemental palette (self-contained for standalone compilation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Wood,
    Water,
    Air,
    Dust,
    Fire,
    Storm,
}

/// A blended element with a strength in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hybrid {
    /// The resulting element.
    pub element: Element,
    /// Blend strength, bounded to `[0, 1]`.
    pub potency: f32,
}

/// Fixed ordering index, used to canonicalize a pair so the table is symmetric.
fn order(e: Element) -> u8 {
    match e {
        Element::Wood => 0,
        Element::Water => 1,
        Element::Air => 2,
        Element::Dust => 3,
        Element::Fire => 4,
        Element::Storm => 5,
    }
}

/// Cross two elements.
///
/// Returns the pure element at full potency when `a == b`; otherwise looks up
/// the canonicalized (index-ordered) pair in the allowed-crossing table.
/// Unlisted pairs return `None`.
pub fn cross(a: Element, b: Element) -> Option<Hybrid> {
    if a == b {
        return Some(Hybrid {
            element: a,
            potency: 1.0,
        });
    }

    // Canonicalize so (a,b) and (b,a) hit the same arm → symmetry by construction.
    let (lo, hi) = if order(a) <= order(b) { (a, b) } else { (b, a) };

    use Element::*;
    let (element, potency) = match (lo, hi) {
        (Wood, Water) => (Wood, 0.70),    // watered growth
        (Water, Air) => (Storm, 0.85),    // the classic storm
        (Air, Fire) => (Storm, 0.75),     // charged updraft
        (Air, Storm) => (Storm, 0.90),    // a gathering gale
        (Water, Storm) => (Storm, 0.80),  // a downpour
        (Dust, Fire) => (Fire, 0.60),     // embers in the dust
        (Wood, Dust) => (Wood, 0.55),     // rooted soil
        _ => return None,
    };
    Some(Hybrid { element, potency })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Element; 6] = [
        Element::Wood,
        Element::Water,
        Element::Air,
        Element::Dust,
        Element::Fire,
        Element::Storm,
    ];

    #[test]
    fn same_element_crosses_to_itself() {
        for e in ALL {
            let h = cross(e, e).expect("same element must cross");
            assert_eq!(h.element, e);
            assert_eq!(h.potency, 1.0);
        }
    }

    #[test]
    fn an_allowed_pair_is_some() {
        let h = cross(Element::Water, Element::Air).expect("water+air allowed");
        assert_eq!(h.element, Element::Storm);
    }

    #[test]
    fn a_disallowed_pair_is_none() {
        // wood + fire is not in the table
        assert_eq!(cross(Element::Wood, Element::Fire), None);
    }

    #[test]
    fn crossing_is_symmetric() {
        for a in ALL {
            for b in ALL {
                assert_eq!(cross(a, b), cross(b, a), "asymmetric for {:?},{:?}", a, b);
            }
        }
    }

    #[test]
    fn potency_is_bounded() {
        for a in ALL {
            for b in ALL {
                if let Some(h) = cross(a, b) {
                    assert!(h.potency >= 0.0 && h.potency <= 1.0, "potency {}", h.potency);
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        for a in ALL {
            for b in ALL {
                assert_eq!(cross(a, b), cross(a, b));
            }
        }
    }

    #[test]
    fn at_least_one_allowed_and_one_disallowed_exist() {
        let mut allowed = 0;
        let mut disallowed = 0;
        for a in ALL {
            for b in ALL {
                if a == b {
                    continue;
                }
                if cross(a, b).is_some() {
                    allowed += 1;
                } else {
                    disallowed += 1;
                }
            }
        }
        assert!(allowed > 0 && disallowed > 0);
    }
}
