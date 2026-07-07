//! body_collision — push overlapping bodies apart, split by mass.
//!
//! Movement is per-creature and surface-based; nothing stopped two creatures
//! standing in the same spot, and a creature parked in a one-wide gap didn't
//! block anyone. This adds the missing rule: two bodies whose centres are closer
//! than a combined `radius` are pushed apart along their line of centres, and the
//! correction is split *inversely to mass* — the heavier one barely moves. A very
//! heavy (or explicitly immovable) body therefore acts as a wall: a lighter
//! creature can't shove through it, so a still mob in a chokepoint blocks the
//! corridor. Pure and deterministic.

/// A mass at or below this is treated as immovable (infinite mass): it never
/// gets pushed, and the other body takes the entire correction.
pub const IMMOVABLE: f32 = 0.0;

/// Resolve overlap between bodies centred at `a` and `b` (XZ), with masses
/// `am`/`bm` and a combined minimum separation `radius`. Returns the corrected
/// `(a, b)`. If they aren't overlapping (`dist >= radius`) both are returned
/// unchanged. The heavier body moves less: `a`'s share of the push is
/// `bm / (am + bm)` (→ 0 as `am` → ∞), so a massive body shoves a light one fully
/// aside. Masses `<= IMMOVABLE` are pinned; if both are pinned, neither moves.
pub fn resolve_overlap(
    a: (f32, f32),
    am: f32,
    b: (f32, f32),
    bm: f32,
    radius: f32,
) -> ((f32, f32), (f32, f32)) {
    let (dx, dz) = (b.0 - a.0, b.1 - a.1);
    let dist2 = dx * dx + dz * dz;
    let r = radius.max(0.0);
    if dist2 >= r * r {
        return (a, b); // not overlapping
    }
    let dist = dist2.sqrt();
    // Degenerate exact overlap: pick a stable axis so the push is deterministic.
    let (nx, nz) = if dist > 1e-6 { (dx / dist, dz / dist) } else { (1.0, 0.0) };
    let overlap = r - dist;

    let a_pinned = am <= IMMOVABLE;
    let b_pinned = bm <= IMMOVABLE;
    let (a_share, b_share) = match (a_pinned, b_pinned) {
        (true, true) => (0.0, 0.0),  // both walls: nothing gives
        (true, false) => (0.0, 1.0), // a is a wall: b takes all
        (false, true) => (1.0, 0.0), // b is a wall: a takes all
        (false, false) => {
            let total = am + bm;
            (bm / total, am / total) // inverse-mass split
        }
    };
    let a2 = (a.0 - nx * overlap * a_share, a.1 - nz * overlap * a_share);
    let b2 = (b.0 + nx * overlap * b_share, b.1 + nz * overlap * b_share);
    (a2, b2)
}

/// Separation distance between two XZ points (convenience for tests/callers).
pub fn separation(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_overlap_unchanged() {
        let (a, b) = resolve_overlap((0.0, 0.0), 1.0, (3.0, 0.0), 1.0, 1.0);
        assert_eq!(a, (0.0, 0.0));
        assert_eq!(b, (3.0, 0.0));
    }

    #[test]
    fn equal_mass_pushes_symmetrically() {
        let (a, b) = resolve_overlap((0.0, 0.0), 1.0, (0.5, 0.0), 1.0, 1.0);
        // Overlap 0.5; each takes half → 0.25 apart from where they were.
        assert!((a.0 - -0.25).abs() < 1e-5, "a.x={}", a.0);
        assert!((b.0 - 0.75).abs() < 1e-5, "b.x={}", b.0);
        assert!((separation(a, b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn heavy_body_barely_moves() {
        let (a, b) = resolve_overlap((0.0, 0.0), 100.0, (0.5, 0.0), 1.0, 1.0);
        let a_moved = a.0.abs();
        let b_moved = (b.0 - 0.5).abs();
        assert!(b_moved > a_moved * 50.0, "light should move far more: a {a_moved} b {b_moved}");
        assert!((separation(a, b) - 1.0).abs() < 1e-5, "still separated to radius");
    }

    #[test]
    fn immovable_body_is_a_wall() {
        // a pinned (mass 0): it must not move; b takes the whole correction.
        let (a, b) = resolve_overlap((5.0, 1.0), IMMOVABLE, (4.4, 1.0), 1.0, 1.0);
        assert_eq!(a, (5.0, 1.0), "wall doesn't move");
        // b was 0.6 away (overlap 0.4), pushed further from a → smaller x.
        assert!(b.0 < 4.4, "b pushed away from the wall, b.x={}", b.0);
        assert!((separation(a, b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn both_immovable_neither_moves() {
        let (a, b) = resolve_overlap((0.0, 0.0), IMMOVABLE, (0.5, 0.0), IMMOVABLE, 1.0);
        assert_eq!(a, (0.0, 0.0));
        assert_eq!(b, (0.5, 0.0));
    }

    #[test]
    fn exact_overlap_separates_deterministically() {
        let (a, b) = resolve_overlap((2.0, 2.0), 1.0, (2.0, 2.0), 1.0, 1.0);
        // Same spot → pushed apart along the fallback +x axis to the full radius.
        assert!((separation(a, b) - 1.0).abs() < 1e-5, "sep={}", separation(a, b));
    }
}
