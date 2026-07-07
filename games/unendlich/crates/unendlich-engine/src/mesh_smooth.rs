//! mesh_smooth.rs — pure chunk-seam blend kernel.
//!
//! A clamped one-ring Laplacian over chunk-edge vertex rings: each vertex's
//! normal is averaged toward its neighbours (and renormalized) and its colour /
//! position blended toward the neighbourhood mean, to hide the cracks and normal
//! discontinuities at chunk boundaries. Pure, deterministic, panic-free
//! (R1/R11/R21). The blend strength `lambda` is clamped to `[0,1]`, so blended
//! values stay in the convex hull of their inputs (no overshoot).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}
fn normalize3(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback // degenerate (opposing normals) -> keep the original
    }
}

/// Smooth a set of vertices given each one's one-ring neighbour indices.
/// `lambda` (clamped to `[0,1]`) is the blend strength toward the neighbourhood
/// mean. Vertices with no neighbours are returned unchanged. Output normals are
/// unit length; output colours/positions lie in the convex hull of each
/// vertex's `{self} ∪ ring`.
pub fn smooth(verts: &[Vertex], one_ring: &[Vec<usize>], lambda: f32) -> Vec<Vertex> {
    let t = lambda.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(verts.len());
    for (i, v) in verts.iter().enumerate() {
        let ring = one_ring.get(i);
        let neighbours = match ring {
            Some(r) if !r.is_empty() => r,
            _ => {
                out.push(*v);
                continue;
            }
        };

        let mut sum_pos = [0.0f32; 3];
        let mut sum_nrm = [0.0f32; 3];
        let mut sum_col = [0.0f32; 4];
        let mut count = 0.0f32;
        for &j in neighbours {
            if j >= verts.len() {
                continue; // ignore out-of-range refs, stay panic-free
            }
            sum_pos = add3(sum_pos, verts[j].pos);
            sum_nrm = add3(sum_nrm, verts[j].normal);
            for k in 0..4 {
                sum_col[k] += verts[j].color[k];
            }
            count += 1.0;
        }
        if count == 0.0 {
            out.push(*v);
            continue;
        }
        let inv = 1.0 / count;
        let mean_pos = scale3(sum_pos, inv);
        let mean_nrm = scale3(sum_nrm, inv);
        let mean_col = [sum_col[0] * inv, sum_col[1] * inv, sum_col[2] * inv, sum_col[3] * inv];

        let pos = mix3(v.pos, mean_pos, t);
        let normal = normalize3(mix3(v.normal, mean_nrm, t), v.normal);
        let color = mix4(v.color, mean_col, t);
        out.push(Vertex { pos, normal, color });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn unit(seed: &mut u32) -> [f32; 3] {
        let v = [lcg(seed) * 2.0 - 1.0, lcg(seed) * 2.0 - 1.0, lcg(seed) * 2.0 - 1.0];
        normalize3(v, [0.0, 1.0, 0.0])
    }

    // A small ring: vertex 0 in the centre, 1..=4 around it; ring of 0 = {1,2,3,4}.
    fn ring_mesh(seed: &mut u32) -> (Vec<Vertex>, Vec<Vec<usize>>) {
        let mut verts = Vec::new();
        for _ in 0..5 {
            verts.push(Vertex {
                pos: [lcg(seed) * 10.0, lcg(seed) * 10.0, lcg(seed) * 10.0],
                normal: unit(seed),
                color: [lcg(seed), lcg(seed), lcg(seed), lcg(seed)],
            });
        }
        let rings = vec![vec![1, 2, 3, 4], vec![0], vec![0], vec![0], vec![0]];
        (verts, rings)
    }

    #[test]
    fn prop_normals_unit_length() {
        let mut seed = 0x5EE_D001u32;
        for _ in 0..200 {
            let (verts, rings) = ring_mesh(&mut seed);
            let lambda = lcg(&mut seed);
            for v in smooth(&verts, &rings, lambda) {
                let len = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "normal not unit: {}", len);
            }
        }
    }

    #[test]
    fn prop_color_in_convex_hull() {
        let mut seed = 0xC010_0001u32;
        for _ in 0..200 {
            let (verts, rings) = ring_mesh(&mut seed);
            let lambda = lcg(&mut seed);
            let out = smooth(&verts, &rings, lambda);
            for (i, ov) in out.iter().enumerate() {
                // inputs for vertex i = {i} ∪ ring(i)
                let mut lo = [f32::INFINITY; 4];
                let mut hi = [f32::NEG_INFINITY; 4];
                let mut consider = |c: [f32; 4]| {
                    for k in 0..4 {
                        lo[k] = lo[k].min(c[k]);
                        hi[k] = hi[k].max(c[k]);
                    }
                };
                consider(verts[i].color);
                for &j in &rings[i] {
                    consider(verts[j].color);
                }
                for k in 0..4 {
                    assert!(
                        ov.color[k] >= lo[k] - 1e-5 && ov.color[k] <= hi[k] + 1e-5,
                        "colour overshoot ch{} = {} not in [{}, {}]",
                        k, ov.color[k], lo[k], hi[k]
                    );
                }
            }
        }
    }

    #[test]
    fn uniform_mesh_is_a_fixed_point() {
        // All identical -> the neighbourhood mean equals self -> nothing moves.
        let v = Vertex { pos: [1.0, 2.0, 3.0], normal: [0.0, 1.0, 0.0], color: [0.2, 0.4, 0.6, 1.0] };
        let verts = vec![v; 5];
        let rings = vec![vec![1, 2, 3, 4], vec![0], vec![0], vec![0], vec![0]];
        let out = smooth(&verts, &rings, 1.0);
        for o in out {
            assert!((o.pos[0] - 1.0).abs() < 1e-6 && (o.pos[1] - 2.0).abs() < 1e-6);
            assert_eq!(o.color, [0.2, 0.4, 0.6, 1.0]);
            assert!((o.normal[1] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn prop_repeat_stays_in_range_contractive() {
        // Repeated smoothing must not blow up: colours stay in the global hull.
        let mut seed = 0x9999_0001u32;
        let (verts, rings) = ring_mesh(&mut seed);
        let mut lo = [f32::INFINITY; 4];
        let mut hi = [f32::NEG_INFINITY; 4];
        for v in &verts {
            for k in 0..4 {
                lo[k] = lo[k].min(v.color[k]);
                hi[k] = hi[k].max(v.color[k]);
            }
        }
        let mut cur = verts;
        for _ in 0..50 {
            cur = smooth(&cur, &rings, 0.5);
            for v in &cur {
                for k in 0..4 {
                    assert!(v.color[k] >= lo[k] - 1e-4 && v.color[k] <= hi[k] + 1e-4, "drift");
                }
                let len = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
                assert!((len - 1.0).abs() < 1e-4 || len < 1e-6, "normal not unit on repeat");
            }
        }
    }

    #[test]
    fn no_neighbours_is_identity() {
        let v = Vertex { pos: [5.0, 6.0, 7.0], normal: [1.0, 0.0, 0.0], color: [0.1, 0.2, 0.3, 0.4] };
        let out = smooth(&[v], &[vec![]], 1.0);
        assert_eq!(out[0], v);
    }

    #[test]
    fn prop_deterministic() {
        let mut s = 0x1234_5678u32;
        let (verts, rings) = ring_mesh(&mut s);
        assert_eq!(smooth(&verts, &rings, 0.5), smooth(&verts, &rings, 0.5));
    }
}
