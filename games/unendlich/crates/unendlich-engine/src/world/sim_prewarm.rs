//! `world::sim_prewarm` — prewarm the *simulation* distance, not the render one.
//!
//! Render distance is what you can see; simulation distance is what the world keeps
//! alive (ticking creatures, fluids, growth) and is usually much larger and more
//! deterministic — it doesn't swing with the camera. So the cheapest way to kill
//! hitches is to generate the sim shell *ahead of the player* before they arrive,
//! at a throttled budget, instead of scrambling to load chunks the instant they
//! enter render range.
//!
//! This planner walks outward ring-by-ring from just beyond the render distance to
//! the sim radius, skips anything already loaded, and within each ring orders the
//! candidates by the **direction of travel** so the chunks the player is heading
//! into warm first. It stops as soon as the per-call budget is met, so it never
//! enumerates the whole sim sphere when only a few chunks are needed.
//!
//! Pure & deterministic (`[i32;3]` coords, no engine deps) so it verifies in
//! isolation; the caller maps to/from `ChunkCoord` and feeds the loader.

/// Plan the next batch of chunks to prewarm toward the simulation radius.
///
/// - `player`: the player's chunk coordinate.
/// - `travel_dir`: an (unnormalised is fine) travel direction; `[0,0,0]` = idle,
///   in which case the walk is purely nearest-first with no forward bias.
/// - `render_radius` / `sim_radius`: chunks in `(render_radius, sim_radius]` are the
///   prewarm zone (they'll simulate before they render). `sim_radius <= render_radius`
///   yields nothing.
/// - `budget`: max chunks to return this call (throttle).
/// - `is_loaded`: skip chunks already present/in-flight.
///
/// Returns up to `budget` coords, nearest ring first and forward-biased within a
/// ring — deterministic for identical inputs.
pub fn plan_prewarm(
    player: [i32; 3],
    travel_dir: [f32; 3],
    render_radius: i32,
    sim_radius: i32,
    budget: usize,
    is_loaded: impl Fn([i32; 3]) -> bool,
) -> Vec<[i32; 3]> {
    let mut out: Vec<[i32; 3]> = Vec::new();
    if budget == 0 || sim_radius <= render_radius {
        return out;
    }
    let dir = normalize(travel_dir);
    let start = render_radius.max(0) + 1;
    let r2_max = sim_radius * sim_radius;

    for r in start..=sim_radius {
        // Collect this Chebyshev shell's unloaded, in-sphere candidates.
        let mut ring: Vec<(f32, [i32; 3])> = Vec::new();
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    // Shell surface only (max axis == r), so each chunk is visited once.
                    if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                        continue;
                    }
                    // Trim the cube corners to a sphere.
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 > r2_max {
                        continue;
                    }
                    let c = [player[0] + dx, player[1] + dy, player[2] + dz];
                    if is_loaded(c) {
                        continue;
                    }
                    // Forward bias: cosine of the chunk direction against travel.
                    // Warmed-sooner = lower score, so more-forward chunks sort first.
                    let fwd = if dir == [0.0, 0.0, 0.0] {
                        0.0
                    } else {
                        let len = (d2 as f32).sqrt().max(1.0);
                        (dx as f32 * dir[0] + dy as f32 * dir[1] + dz as f32 * dir[2]) / len
                    };
                    ring.push((-fwd, c));
                }
            }
        }
        // Order within the ring: most-forward first, then a stable coordinate
        // tiebreak so the plan is fully deterministic.
        ring.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1[0].cmp(&b.1[0]))
                .then(a.1[1].cmp(&b.1[1]))
                .then(a.1[2].cmp(&b.1[2]))
        });
        for (_, c) in ring {
            out.push(c);
            if out.len() >= budget {
                return out;
            }
        }
    }
    out
}

#[inline]
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= 1e-6 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn none(_: [i32; 3]) -> bool {
        false
    }

    #[test]
    fn nothing_to_do_when_sim_not_beyond_render() {
        assert!(plan_prewarm([0, 0, 0], [1.0, 0.0, 0.0], 8, 8, 100, none).is_empty());
        assert!(plan_prewarm([0, 0, 0], [1.0, 0.0, 0.0], 8, 4, 100, none).is_empty());
    }

    #[test]
    fn warms_only_the_shell_beyond_render() {
        // render 2, sim 4 → every returned chunk is at Chebyshev distance 3 or 4.
        let got = plan_prewarm([0, 0, 0], [0.0, 0.0, 0.0], 2, 4, 10_000, none);
        assert!(!got.is_empty());
        for c in &got {
            let cheb = c[0].abs().max(c[1].abs()).max(c[2].abs());
            assert!(cheb == 3 || cheb == 4, "chunk {c:?} outside the prewarm shell");
        }
    }

    #[test]
    fn nearest_ring_comes_first() {
        // With no bias, the whole r=3 ring precedes any r=4 chunk.
        let got = plan_prewarm([0, 0, 0], [0.0, 0.0, 0.0], 2, 4, 10_000, none);
        let first_r4 = got
            .iter()
            .position(|c| c[0].abs().max(c[1].abs()).max(c[2].abs()) == 4);
        let last_r3 = got
            .iter()
            .rposition(|c| c[0].abs().max(c[1].abs()).max(c[2].abs()) == 3);
        if let (Some(f4), Some(l3)) = (first_r4, last_r3) {
            assert!(l3 < f4, "all r=3 chunks must precede the first r=4 chunk");
        }
    }

    #[test]
    fn forward_bias_warms_ahead_first() {
        // Travelling +x: the first prewarmed chunk should be on the +x side.
        let got = plan_prewarm([0, 0, 0], [1.0, 0.0, 0.0], 2, 5, 100, none);
        assert!(got[0][0] > 0, "first prewarm chunk should be ahead (+x): {:?}", got[0]);
        // The +x frontier chunk [3,0,0] should come before the behind chunk [-3,0,0].
        let ahead = got.iter().position(|c| *c == [3, 0, 0]);
        let behind = got.iter().position(|c| *c == [-3, 0, 0]);
        if let (Some(a), Some(b)) = (ahead, behind) {
            assert!(a < b, "ahead must warm before behind");
        }
    }

    #[test]
    fn respects_budget_and_skips_loaded() {
        let loaded: HashSet<[i32; 3]> = [[3, 0, 0], [0, 3, 0]].into_iter().collect();
        let got = plan_prewarm([0, 0, 0], [0.0, 0.0, 0.0], 2, 4, 5, |c| loaded.contains(&c));
        assert_eq!(got.len(), 5, "capped at budget");
        assert!(got.iter().all(|c| !loaded.contains(c)), "loaded chunks skipped");
    }

    #[test]
    fn deterministic() {
        let a = plan_prewarm([1, 2, 3], [0.3, 0.0, 0.7], 3, 6, 40, none);
        let b = plan_prewarm([1, 2, 3], [0.3, 0.0, 0.7], 3, 6, 40, none);
        assert_eq!(a, b);
    }
}
