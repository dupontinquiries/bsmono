//! env_deform — collision-aware deformation. Where a body presses into terrain
//! (a contact normal + a penetration depth), it squashes along that normal and
//! bulges across it — flesh yielding against the ground — composing with the free
//! squash of `deform`, the chain bend of `flex_bend`, and the planted foot of
//! `foot_ik`. No contact ⇒ no deform (identity). OPTIONAL task module. Pure,
//! std-only, deterministic; every output is machine-checkable.

/// How hard the body deforms against contact.
#[derive(Clone, Copy, Debug)]
pub struct ContactParams {
    /// Max fraction a half-extent is squashed along the contact normal (0..1).
    pub max_squash: f32,
    /// Penetration depth (blocks) at which squash saturates.
    pub full_depth: f32,
}

impl Default for ContactParams {
    fn default() -> ContactParams {
        ContactParams { max_squash: 0.35, full_depth: 0.5 }
    }
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-5 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Squash a box's half-extents against a terrain contact. `normal` points out of
/// the surface; `depth` is how far the body has penetrated (`<= 0` means no
/// contact → the half-extents are returned unchanged). The axis most aligned with
/// the normal is compressed by up to `max_squash` (scaled by how deep, saturating
/// at `full_depth`), and the volume freed is pushed into the other two axes
/// (bulge), so the body keeps its mass. Always finite; never collapses past
/// `max_squash`.
pub fn contact_squash(half: [f32; 3], normal: [f32; 3], depth: f32, p: &ContactParams) -> [f32; 3] {
    if depth <= 0.0 {
        return half; // no contact, no deform
    }
    let n = norm3(normal);
    if n == [0.0, 0.0, 0.0] {
        return half;
    }
    let full = p.full_depth.max(1e-4);
    let t = (depth / full).clamp(0.0, 1.0);
    let squash = p.max_squash.clamp(0.0, 0.95) * t; // fraction removed along normal

    // Pick the axis most aligned with the normal as the compression axis.
    let mut axis = 0usize;
    let mut best = n[0].abs();
    if n[1].abs() > best {
        best = n[1].abs();
        axis = 1;
    }
    if n[2].abs() > best {
        axis = 2;
    }

    // Compress that axis; bulge the other two to preserve volume.
    let comp = 1.0 - squash; // in [0.05, 1.0]
    let bulge = if comp > 1e-4 { (1.0 / comp).sqrt() } else { 1.0 };
    let mut out = half;
    for i in 0..3 {
        out[i] = if i == axis { half[i] * comp } else { half[i] * bulge };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn vol(h: [f32; 3]) -> f32 {
        8.0 * h[0] * h[1] * h[2]
    }

    #[test]
    fn no_contact_is_identity() {
        let h = [0.5, 0.4, 0.6];
        let p = ContactParams::default();
        assert_eq!(contact_squash(h, [0.0, 1.0, 0.0], 0.0, &p), h, "depth 0 → unchanged");
        assert_eq!(contact_squash(h, [0.0, 1.0, 0.0], -1.0, &p), h, "negative depth → unchanged");
    }

    #[test]
    fn ground_contact_squashes_vertical_and_bulges_sides() {
        let h = [0.5, 0.5, 0.5];
        let p = ContactParams::default();
        let d = contact_squash(h, [0.0, 1.0, 0.0], 0.5, &p);
        assert!(d[1] < h[1], "Y squashed against the ground");
        assert!(d[0] > h[0] && d[2] > h[2], "X/Z bulge");
        assert!((vol(d) - vol(h)).abs() < 1e-3, "volume roughly preserved");
    }

    #[test]
    fn prop_bounded_and_finite() {
        let p = ContactParams::default();
        let mut seed = 0x00C0_FFEEu32;
        for _ in 0..6000 {
            let h = [0.1 + lcg(&mut seed), 0.1 + lcg(&mut seed), 0.1 + lcg(&mut seed)];
            let n = [lcg(&mut seed) * 2.0 - 1.0, lcg(&mut seed) * 2.0 - 1.0, lcg(&mut seed) * 2.0 - 1.0];
            let depth = lcg(&mut seed) * 2.0 - 0.5;
            let d = contact_squash(h, n, depth, &p);
            for i in 0..3 {
                assert!(d[i].is_finite(), "finite");
                // never compressed past max_squash, never wildly inflated
                assert!(d[i] >= h[i] * (1.0 - p.max_squash) - 1e-6, "axis {i} not over-squashed");
                assert!(d[i] <= h[i] * 4.0 + 1e-6, "axis {i} bounded above");
            }
        }
    }
}
