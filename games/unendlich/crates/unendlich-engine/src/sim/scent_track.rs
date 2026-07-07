//! `sim::scent_track` — follow a decaying scent gradient toward its source.
//!
//! [`step_toward`] looks at the scent in the current cell (`scent_here`) and its
//! four neighbours and returns which neighbour to move into: the strongest one,
//! provided it is stronger than the current cell (move *up* the gradient). When
//! no neighbour improves on the current cell — the trail has decayed away, or the
//! creature is already at a local peak — it returns `4`, meaning the scent is
//! lost. Ties go to the lowest neighbour index, so the choice is stable.
//!
//! Pure and deterministic: a max-scan and a threshold — no clock, no randomness,
//! no cross-imports.

const EPS: f32 = 1e-6;

/// Neighbour index (`0..=3`) of the strongest up-gradient direction, or `4` if
/// the scent is lost (no neighbour beats the current cell / everything ~0).
pub fn step_toward(scent_here: f32, neigh: [f32; 4]) -> u8 {
    let mut best_idx = 0usize;
    let mut best_val = neigh[0];
    let mut i = 1;
    while i < 4 {
        if neigh[i] > best_val {
            best_val = neigh[i];
            best_idx = i;
        }
        i += 1;
    }
    // follow only if the strongest neighbour climbs the gradient and isn't noise
    if best_val > scent_here && best_val > EPS {
        best_idx as u8
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn strongest_neighbor_wins() {
        assert_eq!(step_toward(0.0, [0.2, 0.5, 0.1, 0.0]), 1);
        assert_eq!(step_toward(0.1, [0.05, 0.2, 0.9, 0.3]), 2);
    }

    #[test]
    fn lost_when_all_near_zero() {
        assert_eq!(step_toward(0.0, [0.0, 0.0, 0.0, 0.0]), 4);
        assert_eq!(step_toward(0.0, [1e-9, 1e-9, 1e-9, 1e-9]), 4);
    }

    #[test]
    fn ties_go_to_lowest_index() {
        assert_eq!(step_toward(0.0, [0.5, 0.5, 0.1, 0.0]), 0);
        assert_eq!(step_toward(0.0, [0.1, 0.7, 0.7, 0.2]), 1);
    }

    #[test]
    fn lost_at_local_peak() {
        // current cell stronger than every neighbour → nowhere uphill → lost
        assert_eq!(step_toward(0.9, [0.2, 0.3, 0.1, 0.4]), 4);
    }

    #[test]
    fn decay_handled_returns_bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let here = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let neigh = [
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
            ];
            let r = step_toward(here, neigh);
            assert!(r <= 4, "out of range {}", r);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let here = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let neigh = [
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
                (lcg(&mut seed) % 100_001) as f32 / 100_000.0,
            ];
            assert_eq!(step_toward(here, neigh), step_toward(here, neigh));
        }
    }
}
