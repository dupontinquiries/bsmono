//! `sim::herd_cohesion` — herd steering: stay with the group, but don't crowd.
//!
//! [`herd_steer`] returns a 2D steering vector from the creature's position, the
//! herd centroid, the position of its nearest neighbour, and a separation radius
//! `sep_r`. It pulls toward the centroid (cohesion) and, when the nearest
//! neighbour is inside `sep_r`, pushes away from it (separation) with a strength
//! that grows as they get closer. Separation is weighted to **dominate** inside
//! the radius, so creatures pack loosely but never overlap. At rest — already at
//! the centroid with no one too near — the steer is zero.
//!
//! Pure: vector math with safe normalization, no clock, no randomness, no
//! cross-imports. Output is bounded by `COH_W + SEP_W`.

const EPS: f32 = 1e-6;
const COH_W: f32 = 1.0;
const SEP_W: f32 = 2.0; // separation outweighs cohesion inside sep_r

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn norm2(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}
fn normalize(v: [f32; 2]) -> [f32; 2] {
    let n = norm2(v);
    if n > EPS {
        [v[0] / n, v[1] / n]
    } else {
        [0.0, 0.0]
    }
}

/// Steering vector combining cohesion (toward `centroid`) and separation (away
/// from `nearest` when within `sep_r`).
pub fn herd_steer(
    self_pos: [f32; 2],
    centroid: [f32; 2],
    nearest: [f32; 2],
    sep_r: f32,
) -> [f32; 2] {
    // cohesion: unit pull toward the centroid (zero if already there)
    let coh = normalize(sub(centroid, self_pos));

    // separation: push away from the nearest neighbour, ramping up as it nears
    let to_near = sub(nearest, self_pos);
    let d = norm2(to_near);
    let sep = if sep_r > 0.0 && d > EPS && d < sep_r {
        let strength = (sep_r - d) / sep_r; // (0,1], stronger when closer
        let away = normalize(sub(self_pos, nearest));
        [away[0] * strength, away[1] * strength]
    } else {
        [0.0, 0.0]
    };

    [
        COH_W * coh[0] + SEP_W * sep[0],
        COH_W * coh[1] + SEP_W * sep[1],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_mobs_converge_under_steering() {
        // The roamer tick applies herd_steer as a steering velocity to grouped
        // (herd/social) mobs; N nearby mobs must converge over M ticks while
        // separation keeps them from collapsing onto each other.
        let mut pos: Vec<[f32; 2]> =
            vec![[0.0, 0.0], [10.0, 2.0], [3.0, 9.0], [-6.0, 4.0], [7.0, -5.0]];
        let n = pos.len();
        let sep_r = 2.0f32;
        let dt = 0.2f32;
        let spread = |p: &Vec<[f32; 2]>| {
            let (mut cx, mut cy) = (0.0f32, 0.0f32);
            for q in p {
                cx += q[0];
                cy += q[1];
            }
            cx /= n as f32;
            cy /= n as f32;
            let mut s = 0.0;
            for q in p {
                s += ((q[0] - cx).powi(2) + (q[1] - cy).powi(2)).sqrt();
            }
            s / n as f32
        };
        let s0 = spread(&pos);
        for _ in 0..200 {
            let (mut cx, mut cy) = (0.0f32, 0.0f32);
            for q in &pos {
                cx += q[0];
                cy += q[1];
            }
            cx /= n as f32;
            cy /= n as f32;
            let next: Vec<[f32; 2]> = (0..n)
                .map(|i| {
                    let mut nd = f32::INFINITY;
                    let mut near = pos[i];
                    for j in 0..n {
                        if i == j {
                            continue;
                        }
                        let d = ((pos[j][0] - pos[i][0]).powi(2) + (pos[j][1] - pos[i][1]).powi(2)).sqrt();
                        if d < nd {
                            nd = d;
                            near = pos[j];
                        }
                    }
                    let s = herd_steer(pos[i], [cx, cy], near, sep_r);
                    [pos[i][0] + s[0] * dt, pos[i][1] + s[1] * dt]
                })
                .collect();
            pos = next;
        }
        let s1 = spread(&pos);
        assert!(s1 < s0, "herd should converge inward: {s1} !< {s0}");
        let mut mind = f32::INFINITY;
        for i in 0..n {
            for j in (i + 1)..n {
                mind = mind.min(((pos[i][0] - pos[j][0]).powi(2) + (pos[i][1] - pos[j][1]).powi(2)).sqrt());
            }
        }
        assert!(mind > 0.05, "separation should prevent overlap: {mind}");
    }


    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rf(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 1000.0 - 100.0
    }

    #[test]
    fn pulls_toward_centroid() {
        // no neighbour nearby → pure cohesion toward the centroid
        let s = herd_steer([0.0, 0.0], [10.0, 0.0], [100.0, 100.0], 2.0);
        assert!(s[0] > 0.0, "should pull +x toward centroid, got {:?}", s);
        // points roughly at the centroid direction
        let dot = s[0] * 10.0 + s[1] * 0.0;
        assert!(dot > 0.0);
    }

    #[test]
    fn pushes_from_too_near() {
        // neighbour just to the +x side, within sep_r → push has -x component
        let s = herd_steer([0.0, 0.0], [0.0, 0.0], [0.3, 0.0], 1.0);
        assert!(s[0] < 0.0, "should push -x away from neighbour, got {:?}", s);
    }

    #[test]
    fn zero_at_rest() {
        // already at centroid and nobody within sep_r → no steer
        let s = herd_steer([5.0, 5.0], [5.0, 5.0], [100.0, 100.0], 2.0);
        assert_eq!(s, [0.0, 0.0]);
    }

    #[test]
    fn separation_dominates_inside_sep_r() {
        // centroid and a very-near neighbour both on +x: cohesion pulls +x,
        // separation pushes -x. Separation should win → net -x.
        let s = herd_steer([0.0, 0.0], [10.0, 0.0], [0.1, 0.0], 1.0);
        assert!(s[0] < 0.0, "separation should dominate, got {:?}", s);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let sp = [rf(&mut seed), rf(&mut seed)];
            let c = [rf(&mut seed), rf(&mut seed)];
            let n = [rf(&mut seed), rf(&mut seed)];
            let r = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let s = herd_steer(sp, c, n, r);
            assert!(norm2(s) <= COH_W + SEP_W + 1e-3, "unbounded {:?}", s);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let sp = [rf(&mut seed), rf(&mut seed)];
            let c = [rf(&mut seed), rf(&mut seed)];
            let n = [rf(&mut seed), rf(&mut seed)];
            let r = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            assert_eq!(herd_steer(sp, c, n, r), herd_steer(sp, c, n, r));
        }
    }
}
