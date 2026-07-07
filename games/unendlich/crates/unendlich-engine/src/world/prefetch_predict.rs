//! `prefetch_predict` — chunks to prefetch ahead of player motion.
//!
//! Worker-03 / WORLD pure core, round 22 (GPU_COMPUTE_PLAN #5). From the player's
//! chunk and velocity, returns the priority-ordered chunk coords to pre-fetch /
//! pre-generate along the path of travel, so gen/mesh latency is hidden before
//! the player arrives — smoothing the 235k near-mesh churn from outward
//! exploration. Faster motion reaches farther ahead; a stationary player gets
//! the surrounding ring. Pairs with `chunk_schedule` / `gen_estimate`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const EPS: f32 = 1e-4;

/// Priority-ordered chunk coords to prefetch. `player` is the current chunk,
/// `vel` the velocity, `lookahead` the base reach (scaled by speed), `n` the
/// max count. Moving → samples along the path (closest first); stationary →
/// the surrounding ring.
pub fn prefetch(player: [i32; 3], vel: [f32; 3], lookahead: f32, n: usize) -> Vec<[i32; 3]> {
    let mut out: Vec<[i32; 3]> = Vec::new();
    if n == 0 {
        return out;
    }
    let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
    if speed < EPS {
        push_ring(player, n, &mut out);
        return out;
    }

    let dir = [vel[0] / speed, vel[1] / speed, vel[2] / speed];
    let reach = lookahead.max(0.0) * speed; // faster ⇒ farther ahead
    for i in 0..n {
        let frac = (i + 1) as f32 / n as f32; // (0,1]
        let t = reach * frac;
        let c = [
            player[0] + (dir[0] * t).round() as i32,
            player[1] + (dir[1] * t).round() as i32,
            player[2] + (dir[2] * t).round() as i32,
        ];
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Player chunk + expanding Chebyshev shells, in deterministic order, up to `n`.
fn push_ring(player: [i32; 3], n: usize, out: &mut Vec<[i32; 3]>) {
    out.push(player);
    let mut r = 1i32;
    while out.len() < n && r <= 32 {
        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                        continue; // shell surface only
                    }
                    if out.len() >= n {
                        return;
                    }
                    out.push([player[0] + dx, player[1] + dy, player[2] + dz]);
                }
            }
        }
        r += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist2(a: [i32; 3], b: [i32; 3]) -> i64 {
        let dx = (a[0] - b[0]) as i64;
        let dy = (a[1] - b[1]) as i64;
        let dz = (a[2] - b[2]) as i64;
        dx * dx + dy * dy + dz * dz
    }

    #[test]
    fn faster_reaches_farther() {
        let p = [0, 0, 0];
        let slow = prefetch(p, [1.0, 0.0, 0.0], 10.0, 8);
        let fast = prefetch(p, [10.0, 0.0, 0.0], 10.0, 8);
        let max_slow = slow.iter().map(|&c| dist2(c, p)).max().unwrap();
        let max_fast = fast.iter().map(|&c| dist2(c, p)).max().unwrap();
        assert!(max_fast > max_slow, "faster should prefetch farther");
    }

    #[test]
    fn n_bounds_count() {
        let out = prefetch([5, 5, 5], [1.0, 1.0, 0.0], 20.0, 6);
        assert!(out.len() <= 6);
    }

    #[test]
    fn zero_velocity_is_current_ring() {
        let p = [3, 4, 5];
        let out = prefetch(p, [0.0, 0.0, 0.0], 10.0, 7);
        assert_eq!(out[0], p); // player first
        assert!(out.len() <= 7);
        // includes at least one immediate neighbor
        assert!(out.iter().any(|&c| dist2(c, p) == 1));
    }

    #[test]
    fn coords_are_distinct() {
        let out = prefetch([0, 0, 0], [2.0, 0.0, 0.0], 30.0, 12);
        for i in 0..out.len() {
            for j in (i + 1)..out.len() {
                assert_ne!(out[i], out[j], "duplicate prefetch coord");
            }
        }
    }

    #[test]
    fn ordered_by_closeness_along_path() {
        let p = [0, 0, 0];
        let out = prefetch(p, [5.0, 0.0, 0.0], 20.0, 8);
        // closest-along-path first: the last is the farthest from the player.
        let first = dist2(out[0], p);
        let last = dist2(*out.last().unwrap(), p);
        assert!(last >= first);
    }

    #[test]
    fn deterministic() {
        let a = prefetch([1, 2, 3], [1.0, 2.0, 0.5], 15.0, 9);
        let b = prefetch([1, 2, 3], [1.0, 2.0, 0.5], 15.0, 9);
        assert_eq!(a, b);
    }
}
