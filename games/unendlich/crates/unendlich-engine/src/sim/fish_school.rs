//! fish_school — schooling/boids steering. Classic three rules over a fish's
//! neighborhood: **separation** (steer away from neighbors that are too close),
//! **alignment** (match the local average heading), and **cohesion** (drift toward
//! the local centroid). Returns a steering velocity the caller integrates and
//! clamps to a swim speed. Self-contained: neighbor positions/velocities are
//! passed in (the coordinator hooks the real `neighbor_grid` broadphase). Pure,
//! std-only, deterministic.

/// Weights and radii for the three rules.
#[derive(Clone, Copy, Debug)]
pub struct SchoolParams {
    pub separation: f32,
    pub alignment: f32,
    pub cohesion: f32,
    /// Below this distance a neighbor is repelled (separation).
    pub min_dist: f32,
    /// Maximum magnitude of the returned steering velocity.
    pub max_speed: f32,
}

impl Default for SchoolParams {
    fn default() -> SchoolParams {
        SchoolParams { separation: 1.5, alignment: 1.0, cohesion: 0.8, min_dist: 1.0, max_speed: 4.0 }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn clamp_len(v: [f32; 3], max: f32) -> [f32; 3] {
    let l = len(v);
    if l > max && l > 1e-6 {
        scale(v, max / l)
    } else {
        v
    }
}

/// Compute the steering velocity for one fish given its `pos`/`vel` and the
/// `neighbors` (their positions and velocities, equal length). The result is
/// clamped to `p.max_speed`. With no neighbors the fish keeps its current
/// (clamped) heading.
pub fn steer(pos: [f32; 3], vel: [f32; 3], neighbor_pos: &[[f32; 3]], neighbor_vel: &[[f32; 3]], p: &SchoolParams) -> [f32; 3] {
    let n = neighbor_pos.len().min(neighbor_vel.len());
    if n == 0 {
        return clamp_len(vel, p.max_speed);
    }
    let mut centroid = [0.0; 3];
    let mut avg_vel = [0.0; 3];
    let mut sep = [0.0; 3];
    for i in 0..n {
        centroid = add(centroid, neighbor_pos[i]);
        avg_vel = add(avg_vel, neighbor_vel[i]);
        let away = sub(pos, neighbor_pos[i]);
        let d = len(away);
        if d < p.min_dist && d > 1e-6 {
            // repel, stronger the closer it is
            sep = add(sep, scale(away, (p.min_dist - d) / d));
        }
    }
    let inv = 1.0 / n as f32;
    centroid = scale(centroid, inv);
    avg_vel = scale(avg_vel, inv);

    let cohesion = sub(centroid, pos); // toward the local centre
    let alignment = sub(avg_vel, vel); // match the local heading
    let steering = add(add(scale(sep, p.separation), scale(alignment, p.alignment)), scale(cohesion, p.cohesion));
    clamp_len(add(vel, steering), p.max_speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn cohesion_pulls_toward_the_local_centroid() {
        // neighbors all off to +X; a lone-rule (cohesion only) fish should gain +X.
        let p = SchoolParams { separation: 0.0, alignment: 0.0, cohesion: 1.0, min_dist: 0.0, max_speed: 100.0 };
        let npos = [[10.0, 0.0, 0.0], [12.0, 0.0, 0.0]];
        let nvel = [[0.0; 3], [0.0; 3]];
        let v = steer([0.0; 3], [0.0; 3], &npos, &nvel, &p);
        assert!(v[0] > 0.0, "steers toward the centroid at +X: {v:?}");
    }

    #[test]
    fn separation_repels_a_too_close_neighbor() {
        let p = SchoolParams { separation: 1.0, alignment: 0.0, cohesion: 0.0, min_dist: 2.0, max_speed: 100.0 };
        let npos = [[0.5, 0.0, 0.0]]; // 0.5 away, inside min_dist
        let nvel = [[0.0; 3]];
        let v = steer([0.0; 3], [0.0; 3], &npos, &nvel, &p);
        assert!(v[0] < 0.0, "pushed away from the close neighbor (toward -X): {v:?}");
    }

    #[test]
    fn no_neighbors_keeps_heading_clamped() {
        let p = SchoolParams::default();
        let v = steer([0.0; 3], [100.0, 0.0, 0.0], &[], &[], &p);
        assert!((len(v) - p.max_speed).abs() < 1e-4, "clamped to max_speed");
    }

    #[test]
    fn prop_speed_bounded_and_finite() {
        let p = SchoolParams::default();
        let mut seed = 0x1234_5678u32;
        for _ in 0..4000 {
            let pos = [lcg(&mut seed) * 40.0 - 20.0, lcg(&mut seed) * 40.0 - 20.0, lcg(&mut seed) * 40.0 - 20.0];
            let vel = [lcg(&mut seed) * 10.0 - 5.0, lcg(&mut seed) * 10.0 - 5.0, lcg(&mut seed) * 10.0 - 5.0];
            let m = (lcg(&mut seed) * 8.0) as usize;
            let mut np = Vec::new();
            let mut nv = Vec::new();
            for _ in 0..m {
                np.push([lcg(&mut seed) * 40.0 - 20.0, lcg(&mut seed) * 40.0 - 20.0, lcg(&mut seed) * 40.0 - 20.0]);
                nv.push([lcg(&mut seed) * 10.0 - 5.0, lcg(&mut seed) * 10.0 - 5.0, lcg(&mut seed) * 10.0 - 5.0]);
            }
            let v = steer(pos, vel, &np, &nv, &p);
            assert!(v[0].is_finite() && v[1].is_finite() && v[2].is_finite(), "finite");
            assert!(len(v) <= p.max_speed + 1e-3, "speed bounded: {}", len(v));
        }
    }
}
