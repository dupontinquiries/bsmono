//! `sim::batch_boids` — batched flocking, mirroring `sim::boids` (round 11).
//!
//! [`boid_one`] is the scalar reference: the three-rule steering acceleration
//! for one agent given an explicit neighbour list (the exact math from the r11
//! `boids` core). [`boid_batch`] runs a whole flock at once — for each agent it
//! treats every *other* agent as a neighbour and returns all the accelerations.
//! The batch builds each agent's neighbour list in index order and calls the
//! same scalar kernel, so its output is **bit-for-bit identical** to calling
//! `boid_one` per agent.
//!
//! Perf shape: the batch amortizes one all-pairs sweep over a contiguous agent
//! slice instead of the caller re-gathering a neighbour slice per agent; the
//! inner per-pair math is unchanged, so correctness rides entirely on the
//! shared kernel while the gather/iteration is what gets cheaper.
//!
//! Pure and deterministic: neighbours summed in index order, no clock, no
//! randomness, no cross-imports.

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
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = norm3(v);
    if n > EPS {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Scalar reference: one agent's flocking acceleration given its neighbours.
/// `neigh` is `(position, velocity)` per visible neighbour. Returns
/// `sep*ŝ + ali*â + coh*ĉ`, bounded by `sep + ali + coh`; empty → `[0,0,0]`.
pub fn boid_one(
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

/// Batched: every agent's acceleration treating all other agents as neighbours.
pub fn boid_batch(
    agents: &[([f32; 3], [f32; 3])],
    sep: f32,
    ali: f32,
    coh: f32,
) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(agents.len());
    let mut i = 0;
    while i < agents.len() {
        // gather all other agents, in index order, as this agent's neighbours
        let mut neigh: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(agents.len());
        let mut j = 0;
        while j < agents.len() {
            if j != i {
                neigh.push(agents[j]);
            }
            j += 1;
        }
        out.push(boid_one(agents[i].0, agents[i].1, &neigh, sep, ali, coh));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rf(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 10_000.0 - 10.0
    }
    fn rand_agent(seed: &mut u32) -> ([f32; 3], [f32; 3]) {
        (
            [rf(seed), rf(seed), rf(seed)],
            [rf(seed), rf(seed), rf(seed)],
        )
    }

    // Independent reference: build neighbour lists and call the scalar fn.
    fn reference(
        agents: &[([f32; 3], [f32; 3])],
        sep: f32,
        ali: f32,
        coh: f32,
    ) -> Vec<[f32; 3]> {
        let mut out = Vec::new();
        for i in 0..agents.len() {
            let mut neigh = Vec::new();
            for j in 0..agents.len() {
                if j != i {
                    neigh.push(agents[j]);
                }
            }
            out.push(boid_one(agents[i].0, agents[i].1, &neigh, sep, ali, coh));
        }
        out
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB01D_u32;
        for _ in 0..1500 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut agents = Vec::new();
            for _ in 0..n {
                agents.push(rand_agent(&mut seed));
            }
            let b = boid_batch(&agents, 1.0, 1.0, 1.0);
            let r = reference(&agents, 1.0, 1.0, 1.0);
            assert_eq!(b.len(), r.len());
            for i in 0..b.len() {
                for k in 0..3 {
                    assert_eq!(b[i][k].to_bits(), r[i][k].to_bits());
                }
            }
        }
    }

    #[test]
    fn single_agent_has_no_neighbours() {
        let agents = [([1.0, 2.0, 3.0], [0.1, 0.2, 0.3])];
        let b = boid_batch(&agents, 1.0, 1.0, 1.0);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn empty_neighbours_leave_velocity_unchanged() {
        // scalar path with no neighbours → zero acceleration
        assert_eq!(
            boid_one([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], &[], 1.0, 1.0, 1.0),
            [0.0, 0.0, 0.0]
        );
        // and an empty flock → empty batch
        assert!(boid_batch(&[], 1.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn symmetric_pair_forces_are_opposite() {
        let mut seed = 0x9A1C_u32;
        for _ in 0..2000 {
            let a = rand_agent(&mut seed);
            let b = rand_agent(&mut seed);
            // distinct positions so separation is well-defined
            if a.0 == b.0 {
                continue;
            }
            let out = boid_batch(&[a, b], 1.0, 1.0, 1.0);
            // each agent's only neighbour is the other → exact opposite vectors
            for k in 0..3 {
                assert_eq!(out[0][k], -out[1][k], "axis {} not opposite", k);
            }
        }
    }

    #[test]
    fn bounded_by_gain_sum() {
        let mut seed = 0xB0DD_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 7) as usize;
            let mut agents = Vec::new();
            for _ in 0..n {
                agents.push(rand_agent(&mut seed));
            }
            let sep = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let ali = (lcg(&mut seed) % 1000) as f32 / 100.0;
            let coh = (lcg(&mut seed) % 1000) as f32 / 100.0;
            for acc in boid_batch(&agents, sep, ali, coh) {
                assert!(norm3(acc) <= sep + ali + coh + 1e-3);
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..1500 {
            let n = (lcg(&mut seed) % 6) as usize;
            let mut agents = Vec::new();
            for _ in 0..n {
                agents.push(rand_agent(&mut seed));
            }
            assert_eq!(
                boid_batch(&agents, 1.0, 1.0, 1.0),
                boid_batch(&agents, 1.0, 1.0, 1.0)
            );
        }
    }
}
