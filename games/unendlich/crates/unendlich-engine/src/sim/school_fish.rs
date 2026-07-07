//! `sim::school_fish` — how a fish steers to stay with its school.
//!
//! [`steer`] returns a steering direction from the fish's position, its
//! neighbours' positions, and its own velocity. It blends cohesion (a pull
//! toward the school's centroid) with the fish's current heading (alignment /
//! momentum), so the fish drifts toward the group while keeping its course. With
//! no neighbours it simply keeps its velocity. A school arranged symmetrically
//! around the fish exerts no net pull, so the fish holds its heading.
//!
//! Pure and deterministic: a centroid, two normalizations, and a blend — no
//! clock, no randomness, no cross-imports. The result is unit-ish (≤ 1) for the
//! schooling case; confining the fish to water is left to the caller.

const EPS: f32 = 1e-6;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = norm(v);
    if n > EPS {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Schooling steer direction for a fish.
///
/// Empty `nbrs` returns `vel` unchanged. Otherwise returns
/// `normalize(cohesion_dir + heading_dir)`, blending the pull toward the
/// neighbour centroid with the current heading.
pub fn steer(pos: [f32; 3], nbrs: &[[f32; 3]], vel: [f32; 3]) -> [f32; 3] {
    if nbrs.is_empty() {
        return vel;
    }
    let mut sum = [0.0f32; 3];
    let mut i = 0;
    while i < nbrs.len() {
        sum[0] += nbrs[i][0];
        sum[1] += nbrs[i][1];
        sum[2] += nbrs[i][2];
        i += 1;
    }
    let n = nbrs.len() as f32;
    let centroid = [sum[0] / n, sum[1] / n, sum[2] / n];
    let coh = normalize(sub(centroid, pos));
    let heading = normalize(vel);
    normalize([
        coh[0] + heading[0],
        coh[1] + heading[1],
        coh[2] + heading[2],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn empty_keeps_velocity() {
        let v = [1.0, 2.0, -3.0];
        assert_eq!(steer([0.0, 0.0, 0.0], &[], v), v);
    }

    #[test]
    fn pulls_toward_centroid() {
        // neighbours off in +x; fish moving +y → steer should gain +x
        let s = steer([0.0, 0.0, 0.0], &[[10.0, 0.0, 0.0], [12.0, 0.0, 0.0]], [0.0, 1.0, 0.0]);
        assert!(s[0] > 0.0, "should pull +x toward school, got {:?}", s);
    }

    #[test]
    fn aligns_with_heading() {
        // symmetric school (no cohesion) → steer follows the heading
        let v = [0.0, 0.0, 2.0];
        let s = steer(
            [0.0, 0.0, 0.0],
            &[[5.0, 0.0, 0.0], [-5.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, -5.0, 0.0]],
            v,
        );
        // parallel to the (normalized) heading
        assert!(dot(s, [0.0, 0.0, 1.0]) > 0.99, "should align with heading, got {:?}", s);
    }

    #[test]
    fn bounded_unit_ish() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let pos = [
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
                (lcg(&mut seed) % 2000) as f32 - 1000.0,
            ];
            let n = 1 + (lcg(&mut seed) % 6) as usize;
            let mut nbrs = Vec::new();
            for _ in 0..n {
                nbrs.push([
                    (lcg(&mut seed) % 2000) as f32 - 1000.0,
                    (lcg(&mut seed) % 2000) as f32 - 1000.0,
                    (lcg(&mut seed) % 2000) as f32 - 1000.0,
                ]);
            }
            let vel = [
                (lcg(&mut seed) % 200) as f32 - 100.0,
                (lcg(&mut seed) % 200) as f32 - 100.0,
                (lcg(&mut seed) % 200) as f32 - 100.0,
            ];
            let s = steer(pos, &nbrs, vel);
            assert!(norm(s) <= 1.0 + 1e-4, "unbounded {:?}", s);
        }
    }

    #[test]
    fn symmetric_school_no_net_pull() {
        // neighbours mirrored about the fish → centroid == pos → steer ≈ heading
        let v = [1.0, 0.0, 0.0];
        let s = steer([0.0, 0.0, 0.0], &[[3.0, 4.0, 0.0], [-3.0, -4.0, 0.0]], v);
        assert!((s[0] - 1.0).abs() < 1e-4 && s[1].abs() < 1e-4 && s[2].abs() < 1e-4, "{:?}", s);
    }

    #[test]
    fn deterministic() {
        let pos = [1.0, 2.0, 3.0];
        let nbrs = [[4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let vel = [0.5, -0.5, 1.0];
        assert_eq!(steer(pos, &nbrs, vel), steer(pos, &nbrs, vel));
    }
}
