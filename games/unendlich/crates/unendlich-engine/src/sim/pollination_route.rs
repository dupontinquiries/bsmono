//! `sim::pollination_route` — where a carrier (a bee, a nectar-feeding bird)
//! heads next, which closes the pollination loop: visit a bloom, carry pollen,
//! visit the next.
//!
//! [`next_flower`] picks the nearest flower that is currently **blooming** and
//! **not yet visited**, given the carrier's position, the flowers (each a
//! position and a blooming flag), and a `visited` bitmask (bit `i` set ⇒ flower
//! `i` already done). It returns that flower's index, or `None` when nothing is
//! left to pollinate.
//!
//! Distance is compared as squared Euclidean (no `sqrt` needed to find a
//! minimum), ties resolve to the lowest index, and only the low 64 flowers are
//! tracked by the mask (a deliberate cap matching `u64`). Pure and
//! deterministic: no clock, no randomness.

/// Whether flower `i` is marked visited in the mask (indices ≥ 64 are never
/// considered visited, since the `u64` mask can't track them).
fn is_visited(visited: u64, i: usize) -> bool {
    i < 64 && (visited >> i) & 1 == 1
}

/// Choose the nearest unvisited blooming flower, or `None`.
pub fn next_flower(pos: [f32; 2], flowers: &[([f32; 2], bool)], visited: u64) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    let mut i = 0;
    while i < flowers.len() {
        let (fpos, blooming) = flowers[i];
        if blooming && !is_visited(visited, i) {
            let dx = fpos[0] - pos[0];
            let dy = fpos[1] - pos[1];
            let d2 = dx * dx + dy * dy;
            match best {
                Some((bd, _)) if bd <= d2 => {}
                _ => best = Some((d2, i)),
            }
        }
        i += 1;
    }
    match best {
        Some((_, idx)) => Some(idx),
        None => None,
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
    fn nearest_blooming_is_chosen() {
        let flowers = [
            ([10.0, 0.0], true),
            ([3.0, 0.0], true),
            ([7.0, 0.0], true),
        ];
        assert_eq!(next_flower([0.0, 0.0], &flowers, 0), Some(1));
    }

    #[test]
    fn visited_are_skipped() {
        let flowers = [
            ([3.0, 0.0], true), // nearest, but visited
            ([7.0, 0.0], true),
        ];
        let visited = 1u64 << 0;
        assert_eq!(next_flower([0.0, 0.0], &flowers, visited), Some(1));
    }

    #[test]
    fn non_blooming_are_skipped() {
        let flowers = [
            ([3.0, 0.0], false), // nearest, but not blooming
            ([7.0, 0.0], true),
        ];
        assert_eq!(next_flower([0.0, 0.0], &flowers, 0), Some(1));
    }

    #[test]
    fn none_when_all_done() {
        let flowers = [
            ([3.0, 0.0], true),
            ([7.0, 0.0], false),
        ];
        // flower 0 visited, flower 1 not blooming → nothing left
        let visited = 1u64 << 0;
        assert_eq!(next_flower([0.0, 0.0], &flowers, visited), None);
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(next_flower([1.0, 2.0], &[], 0), None);
    }

    #[test]
    fn ties_go_to_lowest_index() {
        let flowers = [
            ([5.0, 0.0], true),
            ([0.0, 5.0], true), // same distance from origin
        ];
        assert_eq!(next_flower([0.0, 0.0], &flowers, 0), Some(0));
    }

    #[test]
    fn deterministic_and_result_is_valid() {
        let mut seed = 0xF107_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 12) as usize;
            let mut flowers = Vec::new();
            for _ in 0..n {
                let x = (lcg(&mut seed) % 1000) as f32 / 10.0;
                let y = (lcg(&mut seed) % 1000) as f32 / 10.0;
                let blooming = lcg(&mut seed) & 1 == 0;
                flowers.push(([x, y], blooming));
            }
            let pos = [
                (lcg(&mut seed) % 1000) as f32 / 10.0,
                (lcg(&mut seed) % 1000) as f32 / 10.0,
            ];
            let visited = ((lcg(&mut seed) as u64) << 20) ^ lcg(&mut seed) as u64;
            let r = next_flower(pos, &flowers, visited);
            assert_eq!(r, next_flower(pos, &flowers, visited));
            if let Some(idx) = r {
                assert!(idx < flowers.len());
                assert!(flowers[idx].1, "chose a non-blooming flower");
                assert!(!is_visited(visited, idx), "chose a visited flower");
            }
        }
    }
}
