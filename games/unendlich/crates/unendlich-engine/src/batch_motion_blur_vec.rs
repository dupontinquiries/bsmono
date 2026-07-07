//! batch_motion_blur_vec.rs — pure batched motion-blur velocity (microopt of R23).
//!
//! Scalar `vel_one` (mirroring R23 `motion_blur_vec::velocity`: `(cur-prev)·scale`
//! clamped to `±1` per component) plus a batched `vel_batch` that is
//! **bit-for-bit identical** to the scalar path applied per `(prev, cur)` item.
//! Pure, deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: computing velocity for many objects/pixels per frame, the batch
//! hoists only the output allocation (`with_capacity` + a tight loop), running
//! the identical per-item delta·scale·clamp with a shared `scale`, so the
//! velocities match the scalar reference bit-for-bit.

/// Max per-component velocity (clip-space units).
pub const MAX_VEL: f32 = 1.0;

/// Scalar reference: clamped scaled clip-space delta.
pub fn vel_one(prev_clip: [f32; 2], cur_clip: [f32; 2], scale: f32) -> [f32; 2] {
    let vx = (cur_clip[0] - prev_clip[0]) * scale;
    let vy = (cur_clip[1] - prev_clip[1]) * scale;
    [vx.max(-MAX_VEL).min(MAX_VEL), vy.max(-MAX_VEL).min(MAX_VEL)]
}

/// Batched form over `(prev_clip, cur_clip)` items sharing one `scale`, routed
/// through `vel_one` (bit-parity).
pub fn vel_batch(items: &[([f32; 2], [f32; 2])], scale: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(items.len());
    for &(prev, cur) in items {
        out.push(vel_one(prev, cur, scale));
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
    fn bits(v: [f32; 2]) -> [u32; 2] {
        [v[0].to_bits(), v[1].to_bits()]
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0x2557_0001u32;
        let items: Vec<([f32; 2], [f32; 2])> = (0..1000)
            .map(|_| ([lcg(&mut seed), lcg(&mut seed)], [lcg(&mut seed), lcg(&mut seed)]))
            .collect();
        let scale = 1.5f32;
        let batched = vel_batch(&items, scale);
        assert_eq!(batched.len(), items.len());
        for (i, &(p, c)) in items.iter().enumerate() {
            assert_eq!(bits(batched[i]), bits(vel_one(p, c, scale)), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(vel_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = vel_one([0.1, 0.2], [0.3, 0.4], 1.0);
        assert_eq!(bits(vel_batch(&[([0.1, 0.2], [0.3, 0.4])], 1.0)[0]), bits(one));
    }

    #[test]
    fn bounded_clamp() {
        let items = [([0.0f32, 0.0], [100.0f32, -100.0])];
        for v in vel_batch(&items, 5.0) {
            assert!(v[0].abs() <= MAX_VEL && v[1].abs() <= MAX_VEL, "not clamped: {:?}", v);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2557_0002u32;
        let items: Vec<([f32; 2], [f32; 2])> = (0..64)
            .map(|_| ([lcg(&mut seed), lcg(&mut seed)], [lcg(&mut seed), lcg(&mut seed)]))
            .collect();
        assert_eq!(vel_batch(&items, 1.0), vel_batch(&items, 1.0));
    }

    #[test]
    fn static_parity() {
        // a non-moving sample yields zero in both paths
        assert_eq!(bits(vel_batch(&[([0.3, 0.4], [0.3, 0.4])], 2.0)[0]), bits(vel_one([0.3, 0.4], [0.3, 0.4], 2.0)));
    }
}
