//! batch_held_item_pose.rs — pure batched held-item pose (microopt of R16).
//!
//! Scalar `held_pose_one` (mirroring R16 `held_item_pose::held_pose`) plus a
//! batched `held_pose_batch` that is **bit-for-bit identical** to the scalar path
//! applied per `(class, scale)` item. Pure, deterministic, panic-free
//! (R1/R11/R21).
//!
//! Perf note: posing every held item across many entities per frame, the batch
//! hoists only the output allocation (`with_capacity` + a tight loop), running
//! the identical per-item lookup+scale, so the poses match the scalar
//! bit-for-bit.

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pose {
    pub offset: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

const NUM_CLASSES: u8 = 6;

fn base_pose(class: u8) -> ([f32; 3], f32, f32) {
    match class {
        0 => ([0.30, -0.20, 0.45], 0.10, -0.25),
        1 => ([0.28, -0.18, 0.40], 0.05, -0.15),
        2 => ([0.20, -0.10, 0.55], 0.30, 0.00),
        3 => ([0.15, -0.05, 0.30], 0.00, -0.40),
        4 => ([0.35, -0.25, 0.32], -0.20, 0.00),
        5 => ([0.25, -0.15, 0.50], 0.15, -0.10),
        _ => ([0.24, -0.14, 0.38], 0.00, -0.20),
    }
}

/// Scalar reference: hand pose for `class` at `scale` (scale multiplies offset).
pub fn held_pose_one(class: u8, scale: f32) -> Pose {
    let (off, yaw, pitch) = base_pose(class);
    Pose { offset: [off[0] * scale, off[1] * scale, off[2] * scale], yaw, pitch }
}

/// Batched form over `(class, scale)` items. Routes each through
/// `held_pose_one`, guaranteeing bit-parity.
pub fn held_pose_batch(items: &[(u8, f32)]) -> Vec<Pose> {
    let mut out = Vec::with_capacity(items.len());
    for &(class, scale) in items {
        out.push(held_pose_one(class, scale));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(p: Pose) -> [u32; 5] {
        [
            p.offset[0].to_bits(),
            p.offset[1].to_bits(),
            p.offset[2].to_bits(),
            p.yaw.to_bits(),
            p.pitch.to_bits(),
        ]
    }

    fn sample(seed: &mut u32) -> (u8, f32) {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let class = ((*seed >> 8) % 9) as u8; // includes out-of-range -> fallback
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let scale = (*seed >> 8) as f32 / ((1u32 << 24) as f32) * 3.0;
        (class, scale)
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0x1857_0001u32;
        let items: Vec<(u8, f32)> = (0..500).map(|_| sample(&mut seed)).collect();
        let batched = held_pose_batch(&items);
        assert_eq!(batched.len(), items.len());
        for (i, &(c, s)) in items.iter().enumerate() {
            assert_eq!(bits(batched[i]), bits(held_pose_one(c, s)), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(held_pose_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(bits(held_pose_batch(&[(2, 1.5)])[0]), bits(held_pose_one(2, 1.5)));
    }

    #[test]
    fn bounded_finite() {
        let items: Vec<(u8, f32)> = (0..NUM_CLASSES).map(|c| (c, 1.0)).collect();
        for p in held_pose_batch(&items) {
            for v in p.offset {
                assert!(v.is_finite());
            }
            assert!(p.yaw.is_finite() && p.pitch.is_finite());
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x1857_0002u32;
        let items: Vec<(u8, f32)> = (0..64).map(|_| sample(&mut seed)).collect();
        assert_eq!(held_pose_batch(&items), held_pose_batch(&items));
    }

    #[test]
    fn fallback_class_parity() {
        // out-of-range classes still match the scalar fallback bit-for-bit
        assert_eq!(bits(held_pose_batch(&[(200, 1.0)])[0]), bits(held_pose_one(200, 1.0)));
    }
}
