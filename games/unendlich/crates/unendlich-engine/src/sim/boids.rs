//! `sim::boids` — classic three-rule flocking, for bees, songbirds, and other
//! ambient flocks.
//!
//! [`step`] takes a creature's position and velocity plus the positions and
//! velocities of the neighbours it can see, and returns a steering
//! **acceleration** built from the three Reynolds rules:
//!
//! - **separation** — steer away from neighbours that are too close,
//! - **alignment** — steer to match the flock's mean heading,
//! - **cohesion** — steer toward the flock's centroid.
//!
//! Each rule is reduced to a unit direction and scaled by its gain, so the
//! returned acceleration is bounded by `sep + ali + coh` regardless of how the
//! flock is arranged — no runaway forces when two birds nearly overlap. With no
//! neighbours the acceleration is zero (the creature coasts). Pure and
//! deterministic: neighbours are summed in slice order, no clock, no randomness.

const EPS: f32 = 1e-6;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
/// Unit vector in the direction of `v`, or zero if `v` is ~zero.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = norm3(v);
    if n > EPS {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Compute the flocking steering acceleration.
///
/// `neigh` is a slice of `(position, velocity)` for each visible neighbour.
/// Returns `sep*ŝ + ali*â + coh*ĉ` where each hatted term is a unit direction
/// (or zero), so `|result| <= sep + ali + coh`. Empty `neigh` → `[0,0,0]`.
pub fn step(
    self_pos: [f32; 3],
    self_vel: [f32; 3],
    neigh: &[([f32; 3], [f32; 3])],
    sep: f32,
    ali: f32,
    coh: f32,
) -> [f32; 3] {
    if neigh.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let inv_n = 1.0 / neigh.len() as f32;

    let mut sep_acc = [0.0f32, 0.0, 0.0];
    let mut vel_sum = [0.0f32, 0.0, 0.0];
    let mut pos_sum = [0.0f32, 0.0, 0.0];

    for (p, v) in neigh {
        // separation: away from each neighbour, weighted toward the closest
        let away = sub(self_pos, *p);
        let d = norm3(away);
        if d > EPS {
            sep_acc = add(sep_acc, scale(normalize(away), 1.0 / d));
        }
        vel_sum = add(vel_sum, *v);
        pos_sum = add(pos_sum, *p);
    }

    let mean_vel = scale(vel_sum, inv_n);
    let centroid = scale(pos_sum, inv_n);

    let sep_hat = normalize(sep_acc);
    let ali_hat = normalize(sub(mean_vel, self_vel));
    let coh_hat = normalize(sub(centroid, self_pos));

    add(add(scale(sep_hat, sep), scale(ali_hat, ali)), scale(coh_hat, coh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rs(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 10_000.0 - 10.0 // [-10, 10]
    }

    #[test]
    fn empty_neighbours_leave_velocity_unchanged() {
        let a = step([1.0, 2.0, 3.0], [0.5, 0.0, -0.5], &[], 1.0, 1.0, 1.0);
        assert_eq!(a, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn separation_pushes_away_from_a_close_neighbour() {
        // neighbour just to the +x side → acceleration should point -x
        let neigh = [([0.2, 0.0, 0.0], [0.0, 0.0, 0.0])];
        let a = step([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], &neigh, 1.0, 0.0, 0.0);
        assert!(a[0] < 0.0, "should flee -x, got {:?}", a);
    }

    #[test]
    fn alignment_steers_toward_mean_heading() {
        // neighbours all cruising +x; self is still → align to +x
        let neigh = [
            ([5.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([6.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let a = step([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], &neigh, 0.0, 1.0, 0.0);
        assert!(a[0] > 0.0, "should align +x, got {:?}", a);
        assert!(a[1].abs() < 1e-6 && a[2].abs() < 1e-6);
    }

    #[test]
    fn cohesion_pulls_toward_centroid() {
        // neighbours' centroid is at +x → pull +x
        let neigh = [
            ([10.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
            ([10.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        let a = step([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], &neigh, 0.0, 0.0, 1.0);
        assert!(a[0] > 0.0, "should cohere +x, got {:?}", a);
    }

    #[test]
    fn prop_bounded_by_gain_sum() {
        let mut seed = 0xB01D_u32;
        for _ in 0..4000 {
            let sp = [rs(&mut seed), rs(&mut seed), rs(&mut seed)];
            let sv = [rs(&mut seed), rs(&mut seed), rs(&mut seed)];
            let m = (lcg(&mut seed) % 6) as usize;
            let mut neigh = Vec::new();
            for _ in 0..m {
                neigh.push((
                    [rs(&mut seed), rs(&mut seed), rs(&mut seed)],
                    [rs(&mut seed), rs(&mut seed), rs(&mut seed)],
                ));
            }
            let sep = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let ali = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let coh = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let a = step(sp, sv, &neigh, sep, ali, coh);
            assert!(norm3(a) <= sep + ali + coh + 1e-3, "unbounded: {:?}", a);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x7777_u32;
        for _ in 0..2000 {
            let sp = [rs(&mut seed), rs(&mut seed), rs(&mut seed)];
            let sv = [rs(&mut seed), rs(&mut seed), rs(&mut seed)];
            let m = (lcg(&mut seed) % 5) as usize;
            let mut neigh = Vec::new();
            for _ in 0..m {
                neigh.push((
                    [rs(&mut seed), rs(&mut seed), rs(&mut seed)],
                    [rs(&mut seed), rs(&mut seed), rs(&mut seed)],
                ));
            }
            assert_eq!(
                step(sp, sv, &neigh, 1.0, 1.0, 1.0),
                step(sp, sv, &neigh, 1.0, 1.0, 1.0)
            );
        }
    }
}
